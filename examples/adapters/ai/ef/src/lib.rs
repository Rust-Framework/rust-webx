//! Durable usage ledger backed by rust-ef.
//!
//! This crate is intentionally thin: it owns the `ai_usage_events` entity and an
//! `IUsageSink` implementation. Callers provide a `DbContext` factory bound to
//! their configured provider (SQLite/Postgres/MySQL); the sink creates one
//! context per write, so it does not hold a scoped unit-of-work across requests.

use std::sync::Arc;

use async_trait::async_trait;
use rust_ef::prelude::*;
use webx_ai::{now_unix_secs, IUsageSink, UsageError, UsageEvent, UsageStatus};

pub mod api_key;
pub mod worker;

pub use api_key::{ApiKeyEntity, EfApiKeyStore};
pub use worker::{IUsageOutboxHandler, UsageOutboxWorker, UsageRollupWorker};

/// One append-only usage row.
#[derive(Debug, Clone, EntityType)]
#[table("ai_usage_events")]
pub struct UsageEventEntity {
    /// Gateway request id; also the idempotency key.
    #[primary_key]
    #[max_length(64)]
    pub request_id: String,
    /// Owning tenant.
    #[required]
    #[max_length(64)]
    #[index]
    pub tenant_id: String,
    /// API key id (never the secret).
    #[required]
    #[max_length(64)]
    pub key_id: String,
    /// Public model/alias.
    #[required]
    #[max_length(128)]
    pub model: String,
    /// Upstream provider.
    #[required]
    #[max_length(64)]
    pub provider: String,
    /// Prompt tokens.
    #[required]
    pub prompt_tokens: i64,
    /// Completion tokens.
    #[required]
    pub completion_tokens: i64,
    /// Cached prompt tokens.
    #[required]
    pub cached_tokens: i64,
    /// Reasoning tokens.
    #[required]
    pub reasoning_tokens: i64,
    /// Total tokens.
    #[required]
    pub total_tokens: i64,
    /// Cost in micro-USD.
    #[required]
    pub cost_micros: i64,
    /// End-to-end latency.
    #[required]
    pub latency_ms: i32,
    /// Time to first token, when streamed.
    pub ttft_ms: Option<i32>,
    /// `completed` / `client_gone` / `upstream_error`.
    #[required]
    #[max_length(32)]
    pub status: String,
    /// Creation time (Unix seconds).
    #[required]
    #[index]
    pub created_at_unix_secs: i64,
}

impl UsageEventEntity {
    /// Convert an event, stamping `created_at_unix_secs` when absent.
    pub fn from_event(event: UsageEvent) -> Self {
        let created = if event.created_at_unix_secs == 0 {
            now_unix_secs()
        } else {
            event.created_at_unix_secs
        };
        Self {
            request_id: event.request_id,
            tenant_id: event.tenant_id,
            key_id: event.key_id,
            model: event.model,
            provider: event.provider,
            prompt_tokens: to_i64(event.usage.prompt_tokens),
            completion_tokens: to_i64(event.usage.completion_tokens),
            cached_tokens: to_i64(event.usage.cached_tokens),
            reasoning_tokens: to_i64(event.usage.reasoning_tokens),
            total_tokens: to_i64(event.usage.total_tokens()),
            cost_micros: to_i64(event.cost_micros),
            latency_ms: i32::try_from(event.latency_ms).unwrap_or(i32::MAX),
            ttft_ms: event
                .ttft_ms
                .map(|ttft| i32::try_from(ttft).unwrap_or(i32::MAX)),
            status: status_name(event.status).to_string(),
            created_at_unix_secs: to_i64(created),
        }
    }

    /// Convert a row back into a domain event (for rollups/replay).
    pub fn to_event(&self) -> UsageEvent {
        UsageEvent {
            request_id: self.request_id.clone(),
            tenant_id: self.tenant_id.clone(),
            key_id: self.key_id.clone(),
            model: self.model.clone(),
            provider: self.provider.clone(),
            usage: webx_ai::Usage {
                prompt_tokens: u64::try_from(self.prompt_tokens).unwrap_or(0),
                completion_tokens: u64::try_from(self.completion_tokens).unwrap_or(0),
                cached_tokens: u64::try_from(self.cached_tokens).unwrap_or(0),
                reasoning_tokens: u64::try_from(self.reasoning_tokens).unwrap_or(0),
            },
            cost_micros: u64::try_from(self.cost_micros).unwrap_or(0),
            latency_ms: u32::try_from(self.latency_ms).unwrap_or(0),
            ttft_ms: self.ttft_ms.and_then(|ttft| u32::try_from(ttft).ok()),
            status: parse_status(&self.status),
            created_at_unix_secs: u64::try_from(self.created_at_unix_secs).unwrap_or(0),
        }
    }
}

/// Outbox row for external billing/reconciliation.
#[derive(Debug, Clone, EntityType)]
#[table("ai_usage_outbox")]
pub struct UsageOutboxEntity {
    /// Matches the ledger request id.
    #[primary_key]
    #[max_length(64)]
    pub request_id: String,
    /// Serialized `UsageEvent`.
    #[required]
    pub payload: String,
    /// Delivery attempts so far.
    #[required]
    pub attempts: i32,
    /// Earliest next delivery attempt.
    #[required]
    #[index]
    pub next_attempt_at_unix_secs: i64,
    /// Creation time.
    #[required]
    pub created_at_unix_secs: i64,
    /// Whether the payload was delivered.
    #[required]
    #[index]
    pub delivered: bool,
    /// Delivery time.
    pub delivered_at_unix_secs: Option<i64>,
    /// Last delivery error (truncated).
    pub last_error: Option<String>,
}

impl UsageOutboxEntity {
    /// Parse the payload back into an event.
    pub fn event(&self) -> Result<UsageEvent, serde_json::Error> {
        serde_json::from_str(&self.payload)
    }
}

/// Outbox reader/acknowledger over the same database.
pub struct EfUsageOutbox {
    factory: DbContextFactory,
}

impl EfUsageOutbox {
    /// Build an outbox over a `DbContext` factory.
    pub fn new(factory: DbContextFactory) -> Self {
        Self { factory }
    }

    /// Wrap a configured options object.
    pub fn from_options(options: Arc<DbContextOptions>) -> Self {
        Self::new(Arc::new(move || DbContext::from_options(&options)))
    }

    /// Undelivered rows whose retry time has arrived (at most `limit`).
    pub async fn pending(&self, limit: usize) -> EFResult<Vec<UsageOutboxEntity>> {
        let mut ctx = (self.factory)()?;
        let now = to_i64(now_unix_secs());
        let mut rows = linq!(
            ctx.set::<UsageOutboxEntity>(),
            |row: UsageOutboxEntity| row.delivered == false
                && row.next_attempt_at_unix_secs <= now;
            order_by row.created_at_unix_secs
        )
        .to_list()
        .await?;
        rows.truncate(limit);
        Ok(rows)
    }

    /// Mark a row delivered; returns whether it existed.
    pub async fn mark_delivered(&self, request_id: &str) -> EFResult<bool> {
        let mut ctx = (self.factory)()?;
        let Some(mut row) = self.find(&mut ctx, request_id).await? else {
            return Ok(false);
        };
        row.delivered = true;
        row.delivered_at_unix_secs = Some(to_i64(now_unix_secs()));
        ctx.update(row);
        ctx.save_changes().await?;
        Ok(true)
    }

    /// Record a failed attempt with exponential retry delay.
    pub async fn mark_failed(&self, request_id: &str, error: &str) -> EFResult<bool> {
        let mut ctx = (self.factory)()?;
        let Some(mut row) = self.find(&mut ctx, request_id).await? else {
            return Ok(false);
        };
        row.attempts = row.attempts.saturating_add(1);
        let exponent = u32::try_from(row.attempts.min(10)).unwrap_or(0);
        let delay = 2i64.saturating_pow(exponent).saturating_mul(30);
        row.next_attempt_at_unix_secs = to_i64(now_unix_secs()).saturating_add(delay);
        row.last_error = Some(error.chars().take(500).collect());
        ctx.update(row);
        ctx.save_changes().await?;
        Ok(true)
    }

    async fn find(
        &self,
        ctx: &mut DbContext,
        request_id: &str,
    ) -> EFResult<Option<UsageOutboxEntity>> {
        let request_id = request_id.to_string();
        linq!(ctx.set::<UsageOutboxEntity>(), |row: UsageOutboxEntity| row
            .request_id
            == request_id)
        .first_or_default()
        .await
    }
}

/// One persisted rollup window.
#[derive(Debug, Clone, EntityType)]
#[table("ai_usage_rollups")]
pub struct UsageRollupEntity {
    /// `{window_start}:{tenant}:{model}`.
    #[primary_key]
    #[max_length(300)]
    pub id: String,
    /// Inclusive window start.
    #[required]
    #[index]
    pub window_start_unix_secs: i64,
    /// Tenant.
    #[required]
    #[max_length(64)]
    pub tenant_id: String,
    /// Public model/alias.
    #[required]
    #[max_length(128)]
    pub model: String,
    /// Number of requests.
    #[required]
    pub requests: i64,
    /// Prompt tokens.
    #[required]
    pub prompt_tokens: i64,
    /// Completion tokens.
    #[required]
    pub completion_tokens: i64,
    /// Cached tokens.
    #[required]
    pub cached_tokens: i64,
    /// Reasoning tokens.
    #[required]
    pub reasoning_tokens: i64,
    /// Total tokens.
    #[required]
    pub total_tokens: i64,
    /// Cost in micro-USD.
    #[required]
    pub cost_micros: i64,
    /// Creation time.
    #[required]
    pub created_at_unix_secs: i64,
}

impl UsageRollupEntity {
    fn from_rollup(rollup: &webx_ai::UsageRollup) -> Self {
        Self {
            id: format!(
                "{}:{}:{}",
                rollup.window_start_unix_secs, rollup.tenant_id, rollup.model
            ),
            window_start_unix_secs: to_i64(rollup.window_start_unix_secs),
            tenant_id: rollup.tenant_id.clone(),
            model: rollup.model.clone(),
            requests: to_i64(rollup.requests),
            prompt_tokens: to_i64(rollup.usage.prompt_tokens),
            completion_tokens: to_i64(rollup.usage.completion_tokens),
            cached_tokens: to_i64(rollup.usage.cached_tokens),
            reasoning_tokens: to_i64(rollup.usage.reasoning_tokens),
            total_tokens: to_i64(rollup.usage.total_tokens()),
            cost_micros: to_i64(rollup.cost_micros),
            created_at_unix_secs: to_i64(now_unix_secs()),
        }
    }
}

/// Persists one rollup window from the ledger; idempotent per window/tenant/model.
pub struct EfUsageRollup {
    factory: DbContextFactory,
}

impl EfUsageRollup {
    /// Build a rollup writer over a `DbContext` factory.
    pub fn new(factory: DbContextFactory) -> Self {
        Self { factory }
    }

    /// Wrap a configured options object.
    pub fn from_options(options: Arc<DbContextOptions>) -> Self {
        Self::new(Arc::new(move || DbContext::from_options(&options)))
    }

    /// Aggregate `[window_start, window_start + window_secs)` and insert new rows.
    pub async fn rollup_window(
        &self,
        window_start: u64,
        window_secs: u64,
    ) -> EFResult<Vec<UsageRollupEntity>> {
        let mut ctx = (self.factory)()?;
        let start = to_i64(window_start);
        let end = to_i64(window_start.saturating_add(window_secs));
        let rows = linq!(ctx.set::<UsageEventEntity>(), |row: UsageEventEntity| row
            .created_at_unix_secs
            >= start
            && row.created_at_unix_secs < end)
        .to_list()
        .await?;
        let events = rows
            .into_iter()
            .map(|row| row.to_event())
            .collect::<Vec<_>>();
        let rollups = webx_ai::rollup_events(events, window_secs);

        let mut inserted = Vec::new();
        for rollup in rollups {
            let entity = UsageRollupEntity::from_rollup(&rollup);
            let id = entity.id.clone();
            let exists = linq!(ctx.set::<UsageRollupEntity>(), |row: UsageRollupEntity| row
                .id
                == id)
            .first_or_default()
            .await?;
            if exists.is_none() {
                ctx.add(entity.clone());
                inserted.push(entity);
            }
        }
        if !inserted.is_empty() {
            ctx.save_changes().await?;
        }
        Ok(inserted)
    }
}

/// A factory returning a fresh `DbContext` for one write.
pub type DbContextFactory = Arc<dyn Fn() -> EFResult<DbContext> + Send + Sync>;

/// Durable, idempotent `IUsageSink`.
pub struct EfUsageSink {
    factory: DbContextFactory,
    write_outbox: bool,
}

impl EfUsageSink {
    /// Build a sink over a `DbContext` factory; writes the outbox row too.
    pub fn new(factory: DbContextFactory) -> Self {
        Self {
            factory,
            write_outbox: true,
        }
    }

    /// Wrap a configured options object (the common case).
    pub fn from_options(options: Arc<DbContextOptions>) -> Self {
        Self::new(Arc::new(move || DbContext::from_options(&options)))
    }

    /// Skip the outbox row (ledger only).
    pub fn without_outbox(mut self) -> Self {
        self.write_outbox = false;
        self
    }
}

#[async_trait]
impl IUsageSink for EfUsageSink {
    async fn record(&self, event: UsageEvent) -> Result<(), UsageError> {
        if event.request_id.trim().is_empty() {
            return Err(UsageError::Invalid("request_id is required".into()));
        }
        let request_id = event.request_id.clone();
        let mut ctx = (self.factory)().map_err(unavailable)?;

        let existing = linq!(ctx.set::<UsageEventEntity>(), |row: UsageEventEntity| row
            .request_id
            == request_id)
        .first_or_default()
        .await
        .map_err(unavailable)?;
        if existing.is_some() {
            return Ok(());
        }

        let now = now_unix_secs();
        if self.write_outbox {
            let payload = serde_json::to_string(&event)
                .map_err(|error| UsageError::Invalid(format!("usage payload: {error}")))?;
            ctx.add(UsageOutboxEntity {
                request_id: event.request_id.clone(),
                payload,
                attempts: 0,
                next_attempt_at_unix_secs: to_i64(now),
                created_at_unix_secs: to_i64(now),
                delivered: false,
                delivered_at_unix_secs: None,
                last_error: None,
            });
        }
        ctx.add(UsageEventEntity::from_event(event));
        match ctx.save_changes().await {
            Ok(_) => Ok(()),
            // A concurrent writer won the unique key race; first write wins.
            Err(EFError::UniqueViolation { .. }) => Ok(()),
            Err(error) => Err(unavailable(error)),
        }
    }
}

fn unavailable(error: EFError) -> UsageError {
    UsageError::Unavailable(format!("usage ledger: {error}"))
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn status_name(status: UsageStatus) -> &'static str {
    match status {
        UsageStatus::Completed => "completed",
        UsageStatus::ClientGone => "client_gone",
        UsageStatus::UpstreamError => "upstream_error",
    }
}

fn parse_status(status: &str) -> UsageStatus {
    match status {
        "client_gone" => UsageStatus::ClientGone,
        "upstream_error" => UsageStatus::UpstreamError,
        _ => UsageStatus::Completed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use webx_ai::Usage;

    static NEXT_DB: AtomicU64 = AtomicU64::new(0);

    fn test_options() -> (Arc<DbContextOptions>, PathBuf) {
        let sequence = NEXT_DB.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "webx-ai-ef-usage-{}-{}-{}.db",
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

    fn event() -> UsageEvent {
        UsageEvent::new(
            "req-1",
            "tenant-1",
            "key-1",
            "fast",
            "mock",
            Usage::new(3, 2),
            UsageStatus::Completed,
        )
        .with_cost(7)
        .with_latency(12)
        .with_ttft(5)
    }

    #[tokio::test]
    async fn writes_and_reads_a_usage_row() {
        let (options, path) = test_options();
        prepare(&options).await;
        let sink = EfUsageSink::from_options(Arc::clone(&options));
        sink.record(event()).await.expect("record");

        let mut ctx = DbContext::from_options(&options).expect("context");
        let row = linq!(ctx.set::<UsageEventEntity>(), |row: UsageEventEntity| row
            .request_id
            == "req-1")
        .first_or_default()
        .await
        .expect("query")
        .expect("row");
        assert_eq!(row.tenant_id, "tenant-1");
        assert_eq!(row.total_tokens, 5);
        assert_eq!(row.cost_micros, 7);
        assert_eq!(row.ttft_ms, Some(5));
        assert_eq!(row.status, "completed");
        assert!(row.created_at_unix_secs > 0);
        drop(ctx);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn duplicate_request_id_is_idempotent() {
        let (options, path) = test_options();
        prepare(&options).await;
        let sink = EfUsageSink::from_options(Arc::clone(&options));
        sink.record(event()).await.expect("first");
        sink.record(event()).await.expect("duplicate");

        let mut ctx = DbContext::from_options(&options).expect("context");
        let count: i64 = linq!(ctx.set::<UsageEventEntity>(); count)
            .await
            .expect("count");
        assert_eq!(count, 1);
        drop(ctx);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn empty_request_id_is_rejected() {
        let (options, path) = test_options();
        prepare(&options).await;
        let sink = EfUsageSink::from_options(options);
        let mut event = event();
        event.request_id.clear();
        assert!(matches!(
            sink.record(event).await,
            Err(UsageError::Invalid(_))
        ));
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn outbox_row_is_written_and_acknowledged() {
        let (options, path) = test_options();
        prepare(&options).await;
        let sink = EfUsageSink::from_options(Arc::clone(&options));
        sink.record(event()).await.expect("record");

        let outbox = EfUsageOutbox::from_options(Arc::clone(&options));
        let pending = outbox.pending(10).await.expect("pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].event().expect("payload").request_id, "req-1");

        assert!(outbox.mark_delivered("req-1").await.expect("ack"));
        assert!(outbox.pending(10).await.expect("pending").is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn failed_delivery_backs_off() {
        let (options, path) = test_options();
        prepare(&options).await;
        let sink = EfUsageSink::from_options(Arc::clone(&options));
        sink.record(event()).await.expect("record");

        let outbox = EfUsageOutbox::from_options(Arc::clone(&options));
        assert!(outbox
            .mark_failed("req-1", "billing endpoint down")
            .await
            .expect("fail"));
        assert!(
            outbox.pending(10).await.expect("pending").is_empty(),
            "the retry time moved into the future"
        );

        let mut ctx = DbContext::from_options(&options).expect("context");
        let row = linq!(ctx.set::<UsageOutboxEntity>(), |row: UsageOutboxEntity| row
            .request_id
            == "req-1")
        .first_or_default()
        .await
        .expect("query")
        .expect("row");
        assert_eq!(row.attempts, 1);
        assert!(row.next_attempt_at_unix_secs > now_unix_secs() as i64);
        assert_eq!(row.last_error.as_deref(), Some("billing endpoint down"));
        drop(ctx);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn rollup_window_is_idempotent() {
        let (options, path) = test_options();
        prepare(&options).await;
        let sink = EfUsageSink::from_options(Arc::clone(&options));
        // Pin the events to a fixed window instead of relying on "now" being in
        // the same minute as the later rollup call: a real-clock test that
        // crosses a minute boundary between record() and rollup_window() is
        // otherwise flaky (seen on Windows CI).
        let window_start = (now_unix_secs() / 60) * 60;
        sink.record(event().with_timestamp(window_start + 1))
            .await
            .expect("first");

        let mut second = event();
        second.request_id = "req-2".into();
        second.usage = Usage::new(4, 6);
        sink.record(second.with_timestamp(window_start + 2))
            .await
            .expect("second");

        let rollup = EfUsageRollup::from_options(Arc::clone(&options));
        let inserted = rollup
            .rollup_window(window_start, 60)
            .await
            .expect("rollup");
        assert_eq!(inserted.len(), 1);
        assert_eq!(inserted[0].requests, 2);
        assert_eq!(inserted[0].total_tokens, 15);

        let inserted_again = rollup
            .rollup_window(window_start, 60)
            .await
            .expect("rollup");
        assert!(inserted_again.is_empty(), "window is idempotent");
        let _ = std::fs::remove_file(path);
    }
}
