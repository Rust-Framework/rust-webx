//! Background dispatcher that drains the outbox through the event handlers.
//!
//! One tick is one bounded unit of work: open a DI scope, resolve the outbox
//! port, claim up to `batch_size` due rows (lease-based, compare-and-set), and
//! dispatch each row as an [`EventMessage`](webx_core::mediator::EventMessage)
//! through the process's `#[handler] IEventHandler<T>` registrations. A failed
//! tick is logged and the next one runs on schedule (a poller must survive a
//! transient database outage).
//!
//! Delivery is **at-least-once**: the event handler runs before the row is marked
//! delivered, so a crash in between re-delivers the event. Handlers must be
//! idempotent.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rust_dix::{ScopeFactory, ServiceProvider};
use webx_core::error::{Error, Result};
use webx_core::hosted::PeriodicBackgroundService;
use webx_core::mediator::{EventDispatcher, EventHandlerCache};

use crate::ef_outbox::unix_now_secs;
use crate::options::OutboxOptions;
use crate::port::IOutbox;

/// Minimum time between retention cleanups (a full delete on every tick would be
/// wasteful; once an hour is enough for day-granularity retention).
const PURGE_INTERVAL_SECS: i64 = 3_600;

/// Drains the outbox and dispatches events to the application's handlers.
pub struct OutboxDispatcher {
    services: Arc<dyn ScopeFactory>,
    provider: Arc<ServiceProvider>,
    handlers: Arc<EventHandlerCache>,
    options: OutboxOptions,
    last_purge: AtomicI64,
}

impl OutboxDispatcher {
    /// Build a dispatcher over the container's scopes and this process's event
    /// registrations.
    pub fn new(
        services: Arc<dyn ScopeFactory>,
        provider: Arc<ServiceProvider>,
        options: OutboxOptions,
    ) -> Self {
        Self {
            services,
            provider,
            handlers: Arc::new(EventHandlerCache::build()),
            options,
            last_purge: AtomicI64::new(0),
        }
    }

    /// Options this dispatcher was built with.
    pub fn options(&self) -> &OutboxOptions {
        &self.options
    }

    async fn drain(&self) -> Result<()> {
        let scope = self.services.create_scope();
        let outbox = scope.get::<dyn IOutbox>().map_err(|error| {
            Error::Internal(format!(
                "outbox dispatcher needs Arc<dyn IOutbox> registered (call add_outbox): {error}"
            ))
        })?;

        let entries = outbox
            .claim(
                unix_now_secs(),
                self.options.batch_size(),
                self.options.lease_seconds(),
            )
            .await?;
        if entries.is_empty() {
            return Ok(());
        }

        let dispatcher =
            EventDispatcher::with_cache(Arc::clone(&self.provider), Arc::clone(&self.handlers));
        for entry in entries {
            if !dispatcher.has_event(&entry.event_name) {
                // An unregistered event can never succeed: dead-letter it instead
                // of retrying until the retention window ends.
                outbox
                    .dead_letter(
                        &entry.id,
                        &format!(
                            "no #[handler] registration for outbox event `{}`",
                            entry.event_name
                        ),
                    )
                    .await?;
                continue;
            }
            match dispatcher.dispatch(&entry.to_message()).await {
                Ok(()) => outbox.complete(&entry.id).await?,
                Err(error) => {
                    outbox
                        .fail(
                            &entry.id,
                            &error.to_string(),
                            self.options.retry_base_seconds(),
                        )
                        .await?;
                }
            }
        }
        Ok(())
    }

    async fn purge_if_due(&self) -> Result<()> {
        let retention = self.options.retention_seconds();
        if retention == 0 {
            return Ok(());
        }
        let now = unix_now_secs();
        let last = self.last_purge.load(Ordering::Relaxed);
        if now.saturating_sub(last) < PURGE_INTERVAL_SECS {
            return Ok(());
        }
        self.last_purge.store(now, Ordering::Relaxed);

        let scope = self.services.create_scope();
        let outbox = scope.get::<dyn IOutbox>().map_err(|error| {
            Error::Internal(format!("outbox dispatcher needs Arc<dyn IOutbox>: {error}"))
        })?;
        outbox.purge(now.saturating_sub(retention)).await?;
        Ok(())
    }
}

#[async_trait]
impl PeriodicBackgroundService for OutboxDispatcher {
    fn interval(&self) -> Duration {
        self.options.interval()
    }

    async fn tick(&self) -> Result<()> {
        self.drain().await?;
        self.purge_if_due().await
    }
}
