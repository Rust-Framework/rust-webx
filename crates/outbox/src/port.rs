//! Outbox storage port used by the dispatcher.
//!
//! Producers do **not** go through this port: they enqueue rows inside their own
//! transaction (through the storage adapter's `DbContext`/connection helper),
//! which is what makes "business data + event" atomic. The port is only the
//! worker-facing read/claim/complete surface, so a custom store (PostgreSQL
//! `SKIP LOCKED`, Redis, an in-memory test double) can replace a durable one.
//!
//! Every implementation must satisfy the same contract; the reusable suite is in
//! the crate's `testing` module (`testing` feature). It requires that:
//!
//! * `claim` is atomic: two concurrent dispatchers never receive the same row,
//!   and a claimed row is invisible until its lease expires;
//! * `complete` and `dead_letter` are terminal for pending rows;
//! * `fail` increments attempts, applies exponential backoff and dead-letters the
//!   row once the configured maximum is reached;
//! * `purge` only deletes finished (delivered or dead-lettered) rows.

use async_trait::async_trait;
use webx_core::error::Result;
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
