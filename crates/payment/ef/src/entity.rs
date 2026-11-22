//! The durable inbox row and its rust-ef model configuration.
//!
//! The entity is the source of truth for idempotency: `(provider, dedupe_key)`
//! is declared as a **unique** index in [`PaymentWebhookInboxConfiguration`], so
//! two concurrent inserts of the same provider event cannot both succeed. The
//! index is declared here (never as hand-written DDL) and is materialized by the
//! official rust-ef migration tooling — `ensure_created` is for tests only.

use rust_ef::prelude::*;

/// One persisted payment webhook event awaiting business processing.
///
/// Rows move through a small state machine:
///
/// * `processed_at IS NULL AND dead_lettered_at IS NULL` — pending;
/// * claimed by a worker when `next_attempt_at <= now` and the lease
///   (`locked_until`) has expired or was never taken;
/// * `processed_at` set after the handler succeeded;
/// * `dead_lettered_at` set after `max_attempts` failed deliveries.
///
/// `locked_until = 0` means "never claimed" (there is no SQL `NULL` lease: the
/// column is a non-null Unix-seconds timestamp, so the expired-lease test is
/// `locked_until <= now`).
#[derive(Debug, Clone, EntityType)]
#[table("payment_webhook_inbox")]
pub struct PaymentWebhookInbox {
    /// Application-assigned UUID primary key.
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    /// Provider wire name (`stripe`, `alipay`, `wechatpay`, ...).
    #[required]
    #[max_length(64)]
    pub provider: String,
    /// Merchant account the event belongs to, when known.
    #[max_length(64)]
    pub account: Option<String>,
    /// Provider-stable de-duplication key (usually the provider event id).
    #[required]
    #[max_length(255)]
    pub dedupe_key: String,
    /// Normalized event type (`succeeded`, `refund_failed`, ...).
    #[required]
    #[max_length(64)]
    pub event_type: String,
    /// Merchant order id, when the callback carried it.
    #[max_length(255)]
    pub order_id: Option<String>,
    /// Full [`webx_pay::PaymentEvent`] serialized as JSON (bounded by the
    /// webhook body limit at ingress).
    #[required]
    pub payload: String,
    /// Failed delivery attempts so far.
    #[required]
    pub attempts: i32,
    /// Earliest Unix-seconds time for the next delivery attempt.
    #[required]
    pub next_attempt_at: i64,
    /// Lease deadline (Unix seconds); `0` means the row was never claimed.
    #[required]
    pub locked_until: i64,
    /// Unix-seconds time the handler succeeded.
    pub processed_at: Option<i64>,
    /// Unix-seconds time the row was dead-lettered after `max_attempts`.
    pub dead_lettered_at: Option<i64>,
    /// Last delivery error (truncated; never secrets or full provider bodies).
    pub last_error: Option<String>,
    /// Row creation time (Unix seconds).
    #[required]
    pub created_at: i64,
}

impl PaymentWebhookInbox {
    /// The declared unique de-duplication index columns.
    ///
    /// Exposed as a constant so the configuration and tests agree on the exact
    /// `(provider, dedupe_key)` pair without duplicating string literals.
    pub const DEDUPE_INDEX_COLUMNS: &'static [&'static str] =
        &[Self::COLUMN_PROVIDER, Self::COLUMN_DEDUPE_KEY];
}

/// Migration-safe model configuration for [`PaymentWebhookInbox`].
///
/// Registered through `#[entity(PaymentWebhookInbox)]` (inventory), so every
/// `DbContext` built from the default entity model picks it up. The only
/// override is the unique de-duplication index; column constraints stay on the
/// entity fields.
#[derive(Debug, Default)]
pub struct PaymentWebhookInboxConfiguration;

#[rust_ef::entity(PaymentWebhookInbox)]
impl IEntityTypeConfiguration<PaymentWebhookInbox> for PaymentWebhookInboxConfiguration {
    fn configure(&self, entity: &mut EntityTypeBuilder<'_, PaymentWebhookInbox>) {
        entity
            .has_index(PaymentWebhookInbox::DEDUPE_INDEX_COLUMNS)
            .is_unique();
    }
}
