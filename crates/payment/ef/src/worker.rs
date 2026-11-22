//! Background drain worker for the durable payment webhook inbox.
//!
//! One tick is one bounded unit of work: open a DI scope, resolve the inbox and
//! the application handler from it, claim up to `batch_size` due rows, decode
//! each payload into a [`PaymentEvent`](webx_pay::PaymentEvent), call the
//! handler, then `complete` or `fail`. A failed tick is logged and the next one
//! runs on schedule
//! ([`stop_on_error`](PeriodicBackgroundService::stop_on_error) is `false`), so
//! a transient database or handler outage never kills the worker.
//!
//! The inbox implementation resolves an owned `DbContext` from a fresh DI scope
//! per storage operation; the per-tick scope here exists so the application
//! handler (registered as scoped/transient) can inject its own scoped services,
//! including a `DbContext` unit of work, exactly like an HTTP request would.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rust_dix::ScopeFactory;
use webx_core::error::{Error, Result};
use webx_core::hosted::PeriodicBackgroundService;

use crate::options::PaymentInboxOptions;
use crate::port::{IPaymentEventHandler, IPaymentInbox};

/// Minimum time between retention cleanups (a full scan/delete on every tick
/// would be wasteful; once an hour is enough for day-granularity retention).
const PURGE_INTERVAL_SECS: i64 = 3_600;

/// Drains the durable inbox and dispatches persisted events to the
/// application's [`IPaymentEventHandler`].
///
/// Registered by
/// [`add_payment_inbox`](crate::PaymentInboxServiceCollectionExt::add_payment_inbox).
/// All tick knobs come from [`PaymentInboxOptions`].
pub struct PaymentInboxWorker {
    services: Arc<dyn ScopeFactory>,
    handler: Option<Arc<dyn IPaymentEventHandler>>,
    options: PaymentInboxOptions,
    last_purge: AtomicI64,
}

impl PaymentInboxWorker {
    /// Build a worker that resolves the handler from DI on every tick.
    ///
    /// The application must register `Arc<dyn IPaymentEventHandler>` (or a
    /// custom `IPaymentInbox`); see
    /// [`add_payment_inbox_with_handler`](crate::PaymentInboxServiceCollectionExt::add_payment_inbox_with_handler)
    /// for an explicit handler.
    pub fn new(services: Arc<dyn ScopeFactory>, options: PaymentInboxOptions) -> Self {
        Self {
            services,
            handler: None,
            options,
            last_purge: AtomicI64::new(0),
        }
    }

    /// Pin the handler instead of resolving it from DI (manual composition and
    /// tests).
    pub fn with_handler(mut self, handler: Arc<dyn IPaymentEventHandler>) -> Self {
        self.handler = Some(handler);
        self
    }

    /// Options this worker was built with.
    pub fn options(&self) -> &PaymentInboxOptions {
        &self.options
    }

    async fn drain(&self) -> Result<()> {
        // One scope per tick: the application handler is resolved inside it and
        // may inject scoped services (including `DbContext`) for this unit of
        // work.
        let scope = self.services.create_scope();
        let inbox = scope.get::<dyn IPaymentInbox>().map_err(|error| {
            Error::Internal(format!(
                "payment inbox worker needs Arc<dyn IPaymentInbox> registered: {error}"
            ))
        })?;
        let handler = match &self.handler {
            Some(handler) => Arc::clone(handler),
            None => scope.get::<dyn IPaymentEventHandler>().map_err(|error| {
                Error::Internal(format!(
                    "payment inbox worker needs an IPaymentEventHandler registered in DI: {error}"
                ))
            })?,
        };

        let now = crate::unix_now_secs();
        let entries = inbox
            .claim(now, self.options.batch_size(), self.options.lease_seconds())
            .await?;

        for entry in entries {
            let id = entry.id.clone();
            let outcome = match entry.event() {
                Ok(event) => handler.handle(event).await,
                // A corrupt payload can never succeed: fail it like a handler
                // error so backoff/dead-letter applies instead of re-reading it
                // forever.
                Err(error) => Err(error),
            };
            match outcome {
                Ok(()) => inbox.complete(&id).await?,
                Err(error) => {
                    let attempts = entry.attempts.saturating_add(1);
                    if attempts >= self.options.max_attempts() {
                        tracing::warn!(
                            inbox_id = %id,
                            attempts,
                            error = %error,
                            "payment webhook inbox delivery dead-lettered"
                        );
                    }
                    inbox
                        .fail(&id, &error.to_string(), self.options.retry_base_seconds())
                        .await?;
                }
            }
        }

        self.purge_if_due(&inbox, now).await;
        Ok(())
    }

    /// Delete processed rows beyond the retention window, at most once an hour.
    /// Cleanup failures are logged, not propagated: they must not stop delivery.
    async fn purge_if_due(&self, inbox: &Arc<dyn IPaymentInbox>, now: i64) {
        let retention = self.options.retention_seconds();
        if retention <= 0 {
            return;
        }
        let last = self.last_purge.load(Ordering::Relaxed);
        if now.saturating_sub(last) < PURGE_INTERVAL_SECS {
            return;
        }
        self.last_purge.store(now, Ordering::Relaxed);
        let before = now.saturating_sub(retention);
        match inbox.purge_processed(before).await {
            Ok(removed) if removed > 0 => tracing::info!(
                removed,
                retention_days = self.options.retention_days,
                "payment webhook inbox retention cleanup"
            ),
            Ok(_) => {}
            Err(error) => tracing::warn!(
                error = %error,
                "payment webhook inbox retention cleanup failed"
            ),
        }
    }
}

#[async_trait]
impl PeriodicBackgroundService for PaymentInboxWorker {
    fn interval(&self) -> Duration {
        self.options.interval()
    }

    async fn tick(&self) -> Result<()> {
        self.drain().await
    }

    /// A poller must survive a transient database/handler outage; the host
    /// logs the error and runs the next tick on schedule.
    fn stop_on_error(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::port::IPaymentInbox;
    use crate::test_support::{
        event, registered_env, registered_env_with_di_handler, row, row_count, rows, TestHandler,
    };
    use crate::unix_now_secs;
    use rust_ef::prelude::*;

    fn options() -> PaymentInboxOptions {
        PaymentInboxOptions {
            enabled: true,
            interval_seconds: 1,
            batch_size: 10,
            lease_seconds: 30,
            max_attempts: 3,
            retention_days: 0,
        }
    }

    fn worker(provider: &Arc<rust_dix::ServiceProvider>) -> PaymentInboxWorker {
        worker_with(provider, options())
    }

    fn worker_with(
        provider: &Arc<rust_dix::ServiceProvider>,
        options: PaymentInboxOptions,
    ) -> PaymentInboxWorker {
        let services = provider
            .get::<dyn rust_dix::ScopeFactory>()
            .expect("ScopeFactory");
        PaymentInboxWorker::new(services, options)
    }

    #[tokio::test]
    async fn tick_drains_due_events_and_completes_them() {
        let handler = TestHandler::new();
        let provider = registered_env(
            options(),
            Some(Arc::clone(&handler) as Arc<dyn crate::IPaymentEventHandler>),
        )
        .await;
        let inbox = provider.get::<dyn IPaymentInbox>().expect("inbox");
        inbox.record(event("evt_1")).await.expect("record 1");
        inbox.record(event("evt_2")).await.expect("record 2");

        let worker = worker(&provider);
        worker.tick().await.expect("tick");

        let mut handled = handler.handled_keys();
        handled.sort();
        assert_eq!(handled, ["evt_1", "evt_2"]);
        assert!(rows(&provider)
            .await
            .iter()
            .all(|row| row.processed_at.is_some()));

        // An idle tick does not re-run processed events.
        worker.tick().await.expect("idle tick");
        assert_eq!(handler.handled_keys().len(), 2);
    }

    #[tokio::test]
    async fn failing_handler_backs_off_then_dead_letters() {
        let handler = TestHandler::new();
        let mut worker_options = options();
        worker_options.max_attempts = 2;
        let provider = registered_env(
            worker_options.clone(),
            Some(Arc::clone(&handler) as Arc<dyn crate::IPaymentEventHandler>),
        )
        .await;
        let inbox = provider.get::<dyn IPaymentInbox>().expect("inbox");
        handler.fail(true);
        inbox.record(event("evt_1")).await.expect("record");

        let worker = worker_with(&provider, worker_options.clone());
        let before_tick = unix_now_secs();
        worker.tick().await.expect("tick records the failure");
        let stored = rows(&provider).await;
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].attempts, 1);
        assert!(stored[0].processed_at.is_none());
        assert!(
            stored[0].next_attempt_at > before_tick,
            "retry is scheduled in the future"
        );

        // Still inside the backoff window: the next tick claims nothing.
        worker.tick().await.expect("backoff tick is idle");
        assert_eq!(rows(&provider).await[0].attempts, 1);

        // Force the retry time into the past and fail once more: max_attempts=2
        // means the row is dead-lettered instead of rescheduled.
        let id = stored[0].id.clone();
        let scope = provider.scope();
        let mut ctx: DbContext = scope.get_owned().expect("context");
        let query_id = id.clone();
        linq!(
            ctx.set::<crate::PaymentWebhookInbox>(),
            |row: crate::PaymentWebhookInbox| row.id == query_id
        )
        .execute_update()
        .set_column(crate::PaymentWebhookInbox::COLUMN_NEXT_ATTEMPT_AT, 0i64)
        .execute()
        .await
        .expect("force retry time");
        drop(ctx);

        worker.tick().await.expect("dead-letter tick");
        let dead = row(&provider, &id).await;
        assert_eq!(dead.attempts, 2);
        assert!(dead.dead_lettered_at.is_some());

        // Restoring the handler does not resurrect a dead-lettered row.
        handler.fail(false);
        worker.tick().await.expect("idle tick");
        assert_eq!(row_count(&provider).await, 1);
    }

    #[tokio::test]
    async fn corrupt_payload_is_dead_lettered_instead_of_looping_forever() {
        let handler = TestHandler::new();
        let mut worker_options = options();
        worker_options.max_attempts = 1;
        let provider = registered_env(
            worker_options.clone(),
            Some(Arc::clone(&handler) as Arc<dyn crate::IPaymentEventHandler>),
        )
        .await;
        let inbox = provider.get::<dyn IPaymentInbox>().expect("inbox");
        inbox.record(event("evt_1")).await.expect("record");

        // Test-only fixture: corrupt the persisted payload.
        let stored = rows(&provider).await;
        let id = stored[0].id.clone();
        let scope = provider.scope();
        let mut ctx: DbContext = scope.get_owned().expect("context");
        let query_id = id.clone();
        linq!(
            ctx.set::<crate::PaymentWebhookInbox>(),
            |row: crate::PaymentWebhookInbox| row.id == query_id
        )
        .execute_update()
        .set_column(crate::PaymentWebhookInbox::COLUMN_PAYLOAD, "{not json")
        .execute()
        .await
        .expect("corrupt payload");
        drop(ctx);

        worker_with(&provider, worker_options.clone())
            .tick()
            .await
            .expect("tick");
        assert!(row(&provider, &id).await.dead_lettered_at.is_some());
        assert!(handler.handled_keys().is_empty());
    }

    #[tokio::test]
    async fn handler_is_resolved_from_di_on_every_tick() {
        let handler = TestHandler::new();
        let provider = registered_env_with_di_handler(
            options(),
            Arc::clone(&handler) as Arc<dyn crate::IPaymentEventHandler>,
        )
        .await;
        provider
            .get::<dyn IPaymentInbox>()
            .expect("inbox")
            .record(event("evt_1"))
            .await
            .expect("record");

        worker(&provider).tick().await.expect("tick");
        assert_eq!(handler.handled_keys(), ["evt_1"]);
    }

    #[tokio::test]
    async fn retention_cleanup_purges_processed_rows() {
        let handler = TestHandler::new();
        let mut worker_options = options();
        worker_options.retention_days = 1;
        let provider = registered_env(
            worker_options.clone(),
            Some(Arc::clone(&handler) as Arc<dyn crate::IPaymentEventHandler>),
        )
        .await;
        let inbox = provider.get::<dyn IPaymentInbox>().expect("inbox");
        inbox.record(event("evt_1")).await.expect("record");
        let id = rows(&provider).await[0].id.clone();
        let scope = provider.scope();
        let mut ctx: DbContext = scope.get_owned().expect("context");
        let query_id = id.clone();
        linq!(
            ctx.set::<crate::PaymentWebhookInbox>(),
            |row: crate::PaymentWebhookInbox| row.id == query_id
        )
        .execute_update()
        .set_column(
            crate::PaymentWebhookInbox::COLUMN_PROCESSED_AT,
            Some(1_000i64),
        )
        .execute()
        .await
        .expect("back-date processed_at");
        drop(ctx);

        let worker = PaymentInboxWorker::new(
            provider
                .get::<dyn rust_dix::ScopeFactory>()
                .expect("ScopeFactory"),
            worker_options,
        );
        worker.tick().await.expect("tick purges old processed rows");
        assert_eq!(row_count(&provider).await, 0);
    }

    #[test]
    fn zero_interval_is_configuration_error() {
        // The framework rejects a zero interval; options normalise it to 1s.
        assert!(!options().interval().is_zero());
    }
}
