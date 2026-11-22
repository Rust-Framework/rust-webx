//! The durable outbox row.
//!
//! Row state machine:
//!
//! * `delivered_at IS NULL AND dead_lettered_at IS NULL` — pending;
//! * claimed by a dispatcher when `next_attempt_at <= now` and the lease
//!   (`locked_until`) is expired or was never taken;
//! * `delivered_at` is set after the event handlers succeeded;
//! * `dead_lettered_at` is set after `max_attempts` failures (or when no
//!   handler is registered for the event name);
//! * rows are purged by the worker after the retention window.
//!
//! `locked_until = 0` means "never claimed" (there is no SQL `NULL` lease: the
//! column is a non-null Unix-seconds timestamp, so the expired-lease test is
//! `locked_until <= now`). The schema comes from the entity and is materialized
//! by the official rust-ef migration tooling — never hand-written DDL.

use rust_ef::prelude::*;

/// One queued event awaiting delivery.
#[derive(Debug, Clone, EntityType)]
#[table("webx_outbox")]
pub struct OutboxMessage {
    /// Application-assigned primary key (a deterministic id makes producer
    /// retries idempotent: a duplicate insert is a `UniqueViolation`).
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    /// Stable event name (`IEventRequest::event_name`).
    #[required]
    #[max_length(128)]
    #[index]
    pub event_name: String,
    /// JSON payload handed back to the event handlers.
    #[required]
    pub payload: String,
    /// Failed delivery attempts so far.
    #[required]
    pub attempts: i32,
    /// Earliest Unix-seconds time for the next delivery attempt.
    #[required]
    #[index]
    pub next_attempt_at: i64,
    /// Lease deadline (Unix seconds); `0` means the row was never claimed.
    #[required]
    pub locked_until: i64,
    /// Unix-seconds time the event handlers succeeded.
    pub delivered_at: Option<i64>,
    /// Unix-seconds time the row was dead-lettered.
    pub dead_lettered_at: Option<i64>,
    /// Last delivery error (truncated; never secrets or full payloads).
    pub last_error: Option<String>,
    /// Row creation time (Unix seconds).
    #[required]
    pub created_at: i64,
}

/// Register [`OutboxMessage`] in an application `DbContext` model.
///
/// Call it from the same place the application registers its own entities
/// (`ctx.set::<T>()`), before `bootstrap`/`migrate`; the outbox table is then
/// created and migrated by the official rust-ef tooling.
pub fn register_outbox_model(ctx: &mut rust_ef::db_context::DbContext) {
    ctx.set::<OutboxMessage>();
}
