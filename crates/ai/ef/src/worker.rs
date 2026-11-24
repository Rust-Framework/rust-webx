//! Background workers for the usage outbox and rollup windows.
//!
//! Both implement [`PeriodicBackgroundService`], so registering them with
//! `add_background_service_with` gives cancellation, join and error logging for
//! free. Keep ticks bounded (outbox batches are capped; rollups cover one
//! window per tick).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use webx_ai::{now_unix_secs, UsageEvent};
use webx_core::hosted::PeriodicBackgroundService;
use webx_core::{Error, Result};

use crate::{EfUsageOutbox, EfUsageRollup};

/// Delivers one outbox payload to the downstream billing/reconciliation system.
#[async_trait]
pub trait IUsageOutboxHandler: Send + Sync {
    /// Deliver one event; an error is stored and retried with backoff.
    async fn deliver(&self, event: &UsageEvent) -> std::result::Result<(), String>;
}

/// Drains due outbox rows and acknowledges them.
#[derive(Clone)]
pub struct UsageOutboxWorker {
    outbox: Arc<EfUsageOutbox>,
    handler: Arc<dyn IUsageOutboxHandler>,
    limit: usize,
    interval: Duration,
}

impl UsageOutboxWorker {
    /// Build a worker that drains at most `limit` rows per tick.
    pub fn new(
        outbox: Arc<EfUsageOutbox>,
        handler: Arc<dyn IUsageOutboxHandler>,
        limit: usize,
        interval: Duration,
    ) -> Self {
        Self {
            outbox,
            handler,
            limit: limit.max(1),
            interval,
        }
    }
}

#[async_trait]
impl PeriodicBackgroundService for UsageOutboxWorker {
    fn interval(&self) -> Duration {
        self.interval
    }

    async fn tick(&self) -> Result<()> {
        let rows = self
            .outbox
            .pending(self.limit)
            .await
            .map_err(|error| Error::internal(format!("usage outbox read failed: {error}")))?;

        for row in rows {
            let request_id = row.request_id.clone();
            match row.event() {
                Ok(event) => match self.handler.deliver(&event).await {
                    Ok(()) => {
                        self.outbox
                            .mark_delivered(&request_id)
                            .await
                            .map_err(|error| {
                                Error::internal(format!("usage outbox ack failed: {error}"))
                            })?;
                    }
                    Err(error) => {
                        self.outbox
                            .mark_failed(&request_id, &error)
                            .await
                            .map_err(|error| {
                                Error::internal(format!(
                                    "usage outbox failure write failed: {error}"
                                ))
                            })?;
                    }
                },
                Err(error) => {
                    self.outbox
                        .mark_failed(&request_id, &format!("invalid usage payload: {error}"))
                        .await
                        .map_err(|error| {
                            Error::internal(format!("usage outbox failure write failed: {error}"))
                        })?;
                }
            }
        }
        Ok(())
    }
}

/// Rolls up the most recently completed window on every tick.
#[derive(Clone)]
pub struct UsageRollupWorker {
    rollup: Arc<EfUsageRollup>,
    window_secs: u64,
    interval: Duration,
}

impl UsageRollupWorker {
    /// `window` is the aggregation width (minimum 1s); `interval` is the tick period.
    pub fn new(rollup: Arc<EfUsageRollup>, window: Duration, interval: Duration) -> Self {
        Self {
            rollup,
            window_secs: window.as_secs().max(1),
            interval,
        }
    }

    /// The window this worker would roll at `now` (exposed for tests/ops).
    pub fn previous_window_start(&self, now_unix_secs: u64) -> u64 {
        let current = (now_unix_secs / self.window_secs) * self.window_secs;
        current.saturating_sub(self.window_secs)
    }
}

#[async_trait]
impl PeriodicBackgroundService for UsageRollupWorker {
    fn interval(&self) -> Duration {
        self.interval
    }

    async fn tick(&self) -> Result<()> {
        let window_start = self.previous_window_start(now_unix_secs());
        self.rollup
            .rollup_window(window_start, self.window_secs)
            .await
            .map(|_| ())
            .map_err(|error| Error::internal(format!("usage rollup failed: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EfUsageSink;
    use rust_ef::prelude::*;
    use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use webx_ai::{IUsageSink, Usage, UsageStatus};

    static NEXT_DB: AtomicU64 = AtomicU64::new(0);

    fn test_options() -> (Arc<DbContextOptions>, PathBuf) {
        let sequence = NEXT_DB.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "webx-ai-ef-worker-{}-{}-{}.db",
            std::process::id(),
            now_unix_secs(),
            sequence
        ));
        let mut builder = DbContextOptionsBuilder::new();
        builder.use_sqlite(&path.to_string_lossy());
        let options = Arc::new(builder.build());
        options.create_provider().expect("provider");
        (options, path)
    }

    async fn prepare(options: &Arc<DbContextOptions>) {
        let ctx = DbContext::from_options(options).expect("context");
        ctx.ensure_created().await.expect("schema");
    }

    fn event(request_id: &str) -> UsageEvent {
        UsageEvent::new(
            request_id,
            "tenant-1",
            "key-1",
            "fast",
            "mock",
            Usage::new(3, 2),
            UsageStatus::Completed,
        )
        .with_cost(7)
    }

    struct TestHandler {
        fail: AtomicBool,
    }

    #[async_trait]
    impl IUsageOutboxHandler for TestHandler {
        async fn deliver(&self, _event: &UsageEvent) -> std::result::Result<(), String> {
            if self.fail.load(Ordering::Relaxed) {
                Err("billing down".into())
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn outbox_worker_acks_success_and_backs_off_failure() {
        let (options, path) = test_options();
        prepare(&options).await;
        let sink = EfUsageSink::from_options(Arc::clone(&options));
        sink.record(event("req-ok")).await.expect("record ok");
        sink.record(event("req-fail")).await.expect("record fail");

        let outbox = Arc::new(EfUsageOutbox::from_options(Arc::clone(&options)));
        let handler = Arc::new(TestHandler {
            fail: AtomicBool::new(false),
        });
        let worker = UsageOutboxWorker::new(
            Arc::clone(&outbox),
            Arc::clone(&handler) as Arc<dyn IUsageOutboxHandler>,
            10,
            Duration::from_secs(1),
        );

        // First tick delivers both, then the second event is made to fail and
        // is re-queued by the next tick.
        worker.tick().await.expect("deliver");
        assert!(outbox.pending(10).await.expect("pending").is_empty());

        sink.record(event("req-fail-2")).await.expect("record");
        handler.fail.store(true, Ordering::Relaxed);
        worker.tick().await.expect("failure is recorded");
        assert!(
            outbox.pending(10).await.expect("pending").is_empty(),
            "failed row is in backoff"
        );
        let mut ctx = DbContext::from_options(&options).expect("context");
        let row = linq!(
            ctx.set::<crate::UsageOutboxEntity>(),
            |row: crate::UsageOutboxEntity| row.request_id == "req-fail-2"
        )
        .first_or_default()
        .await
        .expect("query")
        .expect("row");
        assert_eq!(row.attempts, 1);
        drop(ctx);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn rollup_worker_processes_the_previous_window() {
        let (options, path) = test_options();
        prepare(&options).await;
        let sink = EfUsageSink::from_options(Arc::clone(&options));

        let now = now_unix_secs();
        let window_secs = 60;
        let previous_start = ((now / window_secs) * window_secs) - window_secs;
        let mut event = event("req-rollup");
        event.created_at_unix_secs = previous_start + 1;
        sink.record(event).await.expect("record");

        let rollup = Arc::new(EfUsageRollup::from_options(Arc::clone(&options)));
        let worker = UsageRollupWorker::new(
            Arc::clone(&rollup),
            Duration::from_secs(window_secs),
            Duration::from_secs(1),
        );
        assert_eq!(worker.previous_window_start(now), previous_start);
        worker.tick().await.expect("rollup");

        let mut ctx = DbContext::from_options(&options).expect("context");
        let rows = linq!(
            ctx.set::<crate::UsageRollupEntity>(),
            |row: crate::UsageRollupEntity| row.window_start_unix_secs == previous_start as i64
        )
        .to_list()
        .await
        .expect("query");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].requests, 1);
        drop(ctx);
        let _ = std::fs::remove_file(path);
    }
}
