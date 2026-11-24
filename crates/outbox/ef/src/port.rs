//! Outbox storage port used by the dispatcher.
//!
//! Producers do **not** go through this port: they enqueue rows inside their own
//! `DbContext` transaction via [`OutboxDbContextExt`](crate::OutboxDbContextExt),
//! which is what makes "business data + event" atomic. The port is only the
//! worker-facing read/claim/complete surface, so a custom store (PostgreSQL
//! `SKIP LOCKED`, Redis) can replace the rust-ef implementation.

use async_trait::async_trait;
use webx_core::error::{Error, Result};
use webx_core::mediator::EventMessage;

/// One claimed outbox row handed to the dispatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEntry {
    /// Row primary key.
    pub id: String,
    /// Stable event name.
    pub event_name: String,
    /// JSON payload recorded by the producer.
    pub payload: String,
    /// Failed attempts before this claim.
    pub attempts: i32,
}

impl OutboxEntry {
    /// Build the wire message dispatched to the event handlers.
    pub fn to_message(&self) -> EventMessage {
        EventMessage::new(&self.event_name, self.payload.as_bytes().to_vec())
    }
}

/// Storage operations used by the background dispatcher.
#[async_trait]
pub trait IOutbox: Send + Sync {
    /// Claim up to `limit` due rows, leasing them for `lease_secs`.
    ///
    /// Claiming must be atomic: two concurrent dispatchers must never both
    /// receive the same row.
    async fn claim(&self, now: i64, limit: usize, lease_secs: i64) -> Result<Vec<OutboxEntry>>;

    /// Mark a claimed row delivered.
    async fn complete(&self, id: &str) -> Result<()>;

    /// Record one failed delivery (increments attempts, applies backoff, ends in
    /// a dead letter after the configured maximum).
    async fn fail(&self, id: &str, error: &str, retry_base_secs: i64) -> Result<()>;

    /// Give up on a row immediately (for example an event with no handler).
    async fn dead_letter(&self, id: &str, error: &str) -> Result<()>;

    /// Delete delivered/dead rows older than `before` (Unix seconds).
    async fn purge(&self, before: i64) -> Result<u64>;
}

/// Map a persistence failure into the framework error surface.
///
/// The text is operator-facing (logs) only: 5xx `Public_detail()` stays `None`.
pub(crate) fn store_error(error: rust_ef::error::EFError) -> Error {
    Error::internal(format!("outbox storage error: {error}"))
}
