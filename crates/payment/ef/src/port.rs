//! Ports the application codes against: the durable inbox and its handler.
//!
//! The webhook path persists first and acknowledges the provider only after
//! [`IPaymentInbox::record`] returned [`RecordOutcome::Inserted`]. Business
//! processing happens later in [`PaymentInboxWorker`](crate::PaymentInboxWorker),
//! which calls the application's [`IPaymentEventHandler`]. Delivery is
//! **at-least-once**: a handler may see the same event twice (for example when
//! a worker dies after the handler succeeded but before `complete` committed),
//! so handlers must be idempotent on `dedupe_key`.

use async_trait::async_trait;
use webx_pay::{PaymentError, PaymentEvent};

/// Result of persisting one webhook event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordOutcome {
    /// A new inbox row was inserted; the provider callback may be acknowledged.
    Inserted,
    /// A row with the same `(provider, dedupe_key)` already exists; nothing was
    /// written and the callback may be acknowledged (the event is already
    /// durable).
    Duplicate,
}

/// A claimed inbox row handed to the worker.
///
/// Only the fields the worker needs are exposed; the payload is the serialized
/// [`PaymentEvent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxEntry {
    /// Inbox row id (pass to [`IPaymentInbox::complete`] / [`IPaymentInbox::fail`]).
    pub id: String,
    /// Failed delivery attempts before this claim.
    pub attempts: i32,
    /// Serialized [`PaymentEvent`] (see [`InboxEntry::event`]).
    pub payload: String,
}

impl InboxEntry {
    /// Decode the persisted [`PaymentEvent`].
    pub fn event(&self) -> Result<PaymentEvent, PaymentError> {
        serde_json::from_str(&self.payload).map_err(|error| {
            PaymentError::Serialization(format!("payment inbox payload is invalid: {error}"))
        })
    }
}

/// Durable webhook inbox: insert-if-new, lease-based claim, ack, retry and
/// retention.
///
/// `now` is an explicit Unix-seconds parameter on [`claim`](Self::claim) and
/// [`purge_processed`](Self::purge_processed) so retry/lease behaviour is
/// deterministic in tests; the implementation only reads the clock for
/// bookkeeping timestamps (`processed_at`, `next_attempt_at`).
#[async_trait]
pub trait IPaymentInbox: Send + Sync {
    /// Persist `event` unless its `(provider, dedupe_key)` pair already exists.
    ///
    /// The unique index is the de-duplication guarantee: a concurrent duplicate
    /// insert surfaces as a unique violation and is reported as
    /// [`RecordOutcome::Duplicate`].
    async fn record(&self, event: PaymentEvent) -> Result<RecordOutcome, PaymentError>;

    /// Lease up to `limit` due rows for `lease_secs` seconds and return them.
    ///
    /// A row is due when it is neither processed nor dead-lettered, its
    /// `next_attempt_at <= now`, and its previous lease (if any) expired
    /// (`locked_until <= now`). Leasing is a compare-and-swap, so two workers
    /// can never claim the same row.
    async fn claim(
        &self,
        now: i64,
        limit: usize,
        lease_secs: i64,
    ) -> Result<Vec<InboxEntry>, PaymentError>;

    /// Mark a claimed row as successfully processed (idempotent).
    async fn complete(&self, id: &str) -> Result<(), PaymentError>;

    /// Record a failed delivery: increments `attempts`, releases the lease and
    /// schedules an exponential-backoff retry, or dead-letters the row when
    /// `attempts` reaches the configured maximum.
    ///
    /// `retry_after` is the **base** retry delay in seconds; the stored delay is
    /// `retry_after * 2^(attempts - 1)`, capped.
    async fn fail(&self, id: &str, error: &str, retry_after: i64) -> Result<(), PaymentError>;

    /// Delete processed rows whose `processed_at` is older than `before`
    /// (retention cleanup). Returns the number of deleted rows.
    async fn purge_processed(&self, before: i64) -> Result<u64, PaymentError>;
}

/// Application business handler for persisted payment events.
///
/// Called by [`PaymentInboxWorker`](crate::PaymentInboxWorker). Returning `Err`
/// schedules a retry with exponential backoff (dead-letter after the configured
/// maximum); implementations must be idempotent because delivery is
/// at-least-once.
#[async_trait]
pub trait IPaymentEventHandler: Send + Sync {
    /// Process one payment event.
    async fn handle(&self, event: PaymentEvent) -> Result<(), PaymentError>;
}
