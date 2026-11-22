//! rust-ef implementation of [`IPaymentInbox`].
//!
//! The inbox is a singleton over the DI container: each storage operation opens
//! a fresh scope and resolves an **owned** `DbContext` from it
//! (`scope.get_owned::<DbContext>()`). Scopes are never shared between awaits or
//! tasks, so the non-`Send` `DbContext` stays a unit-of-work created and dropped
//! inside one operation. The application registers the context with rust-ef's
//! `add_dbcontext` (SQLite/PostgreSQL/MySQL), exactly like any other rust-ef
//! service; this crate never builds a second connection stack.

use std::sync::Arc;

use async_trait::async_trait;
use rust_dix::ScopeFactory;
use rust_ef::prelude::*;
use webx_pay::{PaymentError, PaymentEvent};

use crate::entity::PaymentWebhookInbox;
use crate::port::{IPaymentInbox, InboxEntry, RecordOutcome};

/// Longest `last_error` text persisted on a row.
const MAX_ERROR_CHARS: usize = 500;

/// Maximum backoff exponent (delay is capped at `base * 2^10`).
const MAX_BACKOFF_EXPONENT: u32 = 10;

/// Durable [`IPaymentInbox`] backed by rust-ef.
///
/// Construct it directly in tests/manual compositions, or register it with
/// [`add_payment_inbox`](crate::PaymentInboxServiceCollectionExt::add_payment_inbox),
/// which resolves the rust-dix built-in [`ScopeFactory`].
pub struct EfPaymentInbox {
    services: Arc<dyn ScopeFactory>,
    max_attempts: i32,
}

impl EfPaymentInbox {
    /// Build an inbox over the container's scopes.
    ///
    /// The `DbContext` must be registered as a scoped rust-ef service; the
    /// inbox fails with [`PaymentError::Internal`] (and a readable message) when
    /// it is not.
    pub fn new(services: Arc<dyn ScopeFactory>, max_attempts: i32) -> Self {
        Self {
            services,
            max_attempts: max_attempts.max(1),
        }
    }

    /// The maximum failed deliveries before a row is dead-lettered.
    pub fn max_attempts(&self) -> i32 {
        self.max_attempts
    }

    /// Open a scope and resolve the caller-owned `DbContext` for one operation.
    fn context(&self) -> Result<DbContext, PaymentError> {
        let scope = self.services.create_scope();
        scope.get_owned::<DbContext>().map_err(|error| {
            PaymentError::internal(format!(
                "payment inbox needs a scoped DbContext registered with rust-ef \
                 `add_dbcontext`; resolving it from a fresh DI scope failed: {error}"
            ))
        })
    }
}

#[async_trait]
impl IPaymentInbox for EfPaymentInbox {
    async fn record(&self, event: PaymentEvent) -> Result<RecordOutcome, PaymentError> {
        if event.dedupe_key.trim().is_empty() {
            return Err(PaymentError::validation(
                "a payment event must carry a non-empty dedupe_key",
            ));
        }

        let payload = serde_json::to_string(&event).map_err(|error| {
            PaymentError::Serialization(format!(
                "failed to encode the payment event for the inbox: {error}"
            ))
        })?;
        let now = crate::unix_now_secs();
        let row = PaymentWebhookInbox {
            id: uuid::Uuid::new_v4().to_string(),
            provider: event.provider.as_str().to_string(),
            account: event
                .account
                .as_ref()
                .map(|account| account.as_str().to_string()),
            dedupe_key: event.dedupe_key.clone(),
            event_type: event.event_type.as_str().to_string(),
            order_id: event.order_id.clone(),
            payload,
            attempts: 0,
            next_attempt_at: now,
            locked_until: 0,
            processed_at: None,
            dead_lettered_at: None,
            last_error: None,
            created_at: now,
        };

        let mut ctx = self.context()?;
        ctx.add(row);
        match ctx.save_changes().await {
            Ok(_) => Ok(RecordOutcome::Inserted),
            // The `(provider, dedupe_key)` unique index won the race against a
            // concurrent delivery of the same event: the event is already
            // durable, so the callback may be acknowledged.
            Err(EFError::UniqueViolation { .. }) => Ok(RecordOutcome::Duplicate),
            Err(error) => Err(store_error(error)),
        }
    }

    async fn claim(
        &self,
        now: i64,
        limit: usize,
        lease_secs: i64,
    ) -> Result<Vec<InboxEntry>, PaymentError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let lease_until = now.saturating_add(lease_secs.max(1));
        let mut ctx = self.context()?;

        // Candidate selection is only a hint: the per-row CAS below re-checks
        // every guard, so a row completed or claimed by a concurrent worker in
        // between is skipped instead of being leased twice.
        let candidates = linq!(
            ctx.set::<PaymentWebhookInbox>(),
            |row: PaymentWebhookInbox| row.processed_at.is_null()
                && row.dead_lettered_at.is_null()
                && row.next_attempt_at <= now
                && row.locked_until <= now;
            order_by row.next_attempt_at
        )
        .take(limit)
        .to_list()
        .await
        .map_err(store_error)?;

        let mut claimed = Vec::with_capacity(candidates.len());
        for row in candidates {
            let id = row.id.clone();
            let affected = linq!(
                ctx.set::<PaymentWebhookInbox>(),
                |guard: PaymentWebhookInbox| guard.id == id
                    && guard.processed_at.is_null()
                    && guard.dead_lettered_at.is_null()
                    && guard.next_attempt_at <= now
                    && guard.locked_until <= now
            )
            .execute_update()
            .set_column(PaymentWebhookInbox::COLUMN_LOCKED_UNTIL, lease_until)
            .execute()
            .await
            .map_err(store_error)?;
            if affected == 1 {
                claimed.push(InboxEntry {
                    id: row.id,
                    attempts: row.attempts,
                    payload: row.payload,
                });
            }
        }
        Ok(claimed)
    }

    async fn complete(&self, id: &str) -> Result<(), PaymentError> {
        let mut ctx = self.context()?;
        let target = id.to_string();
        let now = crate::unix_now_secs();
        linq!(
            ctx.set::<PaymentWebhookInbox>(),
            |row: PaymentWebhookInbox| row.id == target && row.processed_at.is_null()
        )
        .execute_update()
        .set_column(PaymentWebhookInbox::COLUMN_PROCESSED_AT, Some(now))
        .set_column(PaymentWebhookInbox::COLUMN_LOCKED_UNTIL, 0i64)
        .execute()
        .await
        .map_err(store_error)?;
        Ok(())
    }

    async fn fail(&self, id: &str, error: &str, retry_after: i64) -> Result<(), PaymentError> {
        let mut ctx = self.context()?;
        let target = id.to_string();
        let query_id = target.clone();
        let Some(row) = linq!(
            ctx.set::<PaymentWebhookInbox>(),
            |row: PaymentWebhookInbox| row.id == query_id
        )
        .first_or_default()
        .await
        .map_err(store_error)?
        else {
            return Ok(());
        };
        if row.processed_at.is_some() || row.dead_lettered_at.is_some() {
            // A late failure report for an already-finished row is a no-op.
            return Ok(());
        }

        let now = crate::unix_now_secs();
        let message = truncate(error, MAX_ERROR_CHARS);
        let attempts = row.attempts.saturating_add(1);
        let dead_lettered = attempts >= self.max_attempts;
        let next_attempt_at = if dead_lettered {
            row.next_attempt_at
        } else {
            // 2^(attempts - 1) base delay: base, 2x, 4x, ... capped.
            let exponent =
                u32::try_from(attempts.saturating_sub(1).min(MAX_BACKOFF_EXPONENT as i32))
                    .unwrap_or(0);
            let delay = retry_after
                .max(1)
                .saturating_mul(2i64.saturating_pow(exponent));
            now.saturating_add(delay)
        };

        // The row is leased by this worker, so it is safe to mutate the attached
        // entity and let `detect_changes` write only the changed columns instead
        // of overwriting the whole row (`ctx.update`) and racing unrelated
        // writers.
        let provider = row.provider.clone();
        ctx.attach(row);
        let Some(tracked) = ctx
            .set::<PaymentWebhookInbox>()
            .tracked_entries_mut()
            .find(|row| row.id == target)
        else {
            return Ok(());
        };
        tracked.attempts = attempts;
        tracked.locked_until = 0;
        tracked.last_error = Some(message.clone());
        tracked.next_attempt_at = next_attempt_at;
        if dead_lettered {
            tracked.dead_lettered_at = Some(now);
        }
        ctx.detect_changes();
        ctx.save_changes().await.map_err(store_error)?;

        if dead_lettered {
            tracing::warn!(
                inbox_id = %target,
                provider = %provider,
                attempts,
                error = %message,
                "payment webhook inbox row dead-lettered"
            );
        }
        Ok(())
    }

    async fn purge_processed(&self, before: i64) -> Result<u64, PaymentError> {
        let mut ctx = self.context()?;
        linq!(
            ctx.set::<PaymentWebhookInbox>(),
            |row: PaymentWebhookInbox| row.processed_at.is_not_null() && row.processed_at < before
        )
        .execute_delete()
        .await
        .map_err(store_error)
    }
}

/// Map a persistence failure into the payment domain.
///
/// The error text is operator-facing (logs) only: [`PaymentError::Internal`]
/// never exposes storage details to an HTTP client (`public_detail()` is
/// `None`), and provider callbacks see a 5xx so they retry.
fn store_error(error: EFError) -> PaymentError {
    PaymentError::internal(format!("payment inbox storage error: {error}"))
}

/// Cap `text` at `max` characters on a character boundary.
fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{direct_env, event, row, row_count, rows};
    use crate::unix_now_secs;
    use webx_pay::ProviderId;

    #[tokio::test]
    async fn record_inserts_and_duplicate_is_detected() {
        let (provider, inbox) = direct_env(3).await;

        assert_eq!(
            inbox.record(event("evt_1")).await.expect("first"),
            RecordOutcome::Inserted
        );
        assert_eq!(
            inbox.record(event("evt_1")).await.expect("duplicate"),
            RecordOutcome::Duplicate
        );
        assert_eq!(
            row_count(&provider).await,
            1,
            "only the first delivery is stored"
        );

        let stored = rows(&provider).await;
        assert_eq!(stored[0].provider, "stripe");
        assert_eq!(stored[0].account.as_deref(), Some("acct_1"));
        assert_eq!(stored[0].event_type, "succeeded");
        assert_eq!(stored[0].order_id.as_deref(), Some("ORDER-1"));
        assert_eq!(stored[0].attempts, 0);
        assert!(stored[0].processed_at.is_none());
        assert!(stored[0].dead_lettered_at.is_none());

        // The unique key is `(provider, dedupe_key)`: the same provider event id
        // at another provider is a different event.
        let mut other_provider = event("evt_1");
        other_provider.provider = ProviderId::Alipay;
        assert_eq!(
            inbox.record(other_provider).await.expect("other provider"),
            RecordOutcome::Inserted
        );
        assert_eq!(row_count(&provider).await, 2);
    }

    #[tokio::test]
    async fn empty_dedupe_key_is_rejected_before_touching_storage() {
        let (provider, inbox) = direct_env(3).await;
        assert!(matches!(
            inbox.record(event("   ")).await,
            Err(PaymentError::Validation { .. })
        ));
        assert_eq!(row_count(&provider).await, 0);
    }

    #[tokio::test]
    async fn claim_leases_rows_and_prevents_double_claim() {
        let (provider, inbox) = direct_env(3).await;
        inbox.record(event("evt_1")).await.expect("record 1");
        inbox.record(event("evt_2")).await.expect("record 2");

        let now = unix_now_secs();
        let mut claimed_keys: Vec<String> = inbox
            .claim(now, 10, 30)
            .await
            .expect("claim")
            .iter()
            .map(|entry| entry.event().expect("payload").dedupe_key)
            .collect();
        claimed_keys.sort();
        assert_eq!(claimed_keys, ["evt_1", "evt_2"]);
        for row in rows(&provider).await {
            assert_eq!(row.locked_until, now + 30, "lease is stamped on every row");
        }

        // The lease is still valid: another tick cannot claim the same rows.
        assert!(inbox
            .claim(now + 1, 10, 30)
            .await
            .expect("claim during lease")
            .is_empty());

        // After the lease expires the rows become claimable again.
        let reclaimed = inbox
            .claim(now + 31, 10, 30)
            .await
            .expect("claim after lease");
        assert_eq!(reclaimed.len(), 2, "expired leases are reclaimable");
    }

    #[tokio::test]
    async fn claim_honours_the_batch_limit() {
        let (_provider, inbox) = direct_env(3).await;
        for key in ["evt_1", "evt_2", "evt_3"] {
            inbox.record(event(key)).await.expect("record");
        }
        let claimed = inbox.claim(unix_now_secs(), 2, 30).await.expect("claim");
        assert_eq!(claimed.len(), 2);
    }

    #[tokio::test]
    async fn complete_marks_processed_and_is_idempotent() {
        let (provider, inbox) = direct_env(3).await;
        inbox.record(event("evt_1")).await.expect("record");

        let now = unix_now_secs();
        let claimed = inbox.claim(now, 10, 30).await.expect("claim");
        assert_eq!(claimed.len(), 1);
        let id = claimed[0].id.clone();

        inbox.complete(&id).await.expect("complete");
        let stored = row(&provider, &id).await;
        assert!(stored.processed_at.is_some());
        assert_eq!(stored.locked_until, 0, "complete releases the lease");

        // Completing twice and claiming again are both no-ops.
        inbox.complete(&id).await.expect("idempotent complete");
        assert!(inbox
            .claim(now + 3600, 10, 30)
            .await
            .expect("claim")
            .is_empty());
    }

    #[tokio::test]
    async fn fail_backs_off_then_dead_letters_after_max_attempts() {
        let (provider, inbox) = direct_env(2).await;
        inbox.record(event("evt_1")).await.expect("record");

        let now = unix_now_secs();
        let claimed = inbox.claim(now, 10, 30).await.expect("claim");
        let id = claimed[0].id.clone();

        let before_first = unix_now_secs();
        inbox.fail(&id, "provider down", 2).await.expect("fail");
        let after_first = unix_now_secs();
        let first = row(&provider, &id).await;
        assert_eq!(first.attempts, 1);
        assert_eq!(first.last_error.as_deref(), Some("provider down"));
        assert_eq!(first.locked_until, 0, "failure releases the lease");
        assert!(
            first.next_attempt_at >= before_first + 2 && first.next_attempt_at <= after_first + 2,
            "first retry uses the base delay (got {})",
            first.next_attempt_at
        );
        assert!(first.dead_lettered_at.is_none());

        // A row in backoff is not claimable before `next_attempt_at`.
        assert!(inbox
            .claim(after_first + 1, 10, 30)
            .await
            .expect("claim")
            .is_empty());

        // The second failure reaches `max_attempts` and dead-letters the row.
        inbox
            .fail(&id, "provider down again", 2)
            .await
            .expect("fail");
        let second = row(&provider, &id).await;
        assert_eq!(second.attempts, 2);
        assert!(second.dead_lettered_at.is_some(), "row is dead-lettered");
        assert!(inbox
            .claim(after_first + 86_400, 10, 30)
            .await
            .expect("claim")
            .is_empty());
    }

    #[tokio::test]
    async fn fail_doubles_the_retry_delay_per_attempt() {
        let (provider, inbox) = direct_env(6).await;
        inbox.record(event("evt_1")).await.expect("record");
        let id = inbox.claim(unix_now_secs(), 10, 30).await.expect("claim")[0]
            .id
            .clone();

        let before_first = unix_now_secs();
        inbox.fail(&id, "boom", 2).await.expect("fail");
        let after_first = unix_now_secs();
        let first = row(&provider, &id).await;
        assert!(first.next_attempt_at >= before_first + 2);
        assert!(first.next_attempt_at <= after_first + 2);

        let before_second = unix_now_secs();
        inbox.fail(&id, "boom", 2).await.expect("fail");
        let after_second = unix_now_secs();
        let second = row(&provider, &id).await;
        assert!(
            second.next_attempt_at >= before_second + 4,
            "second retry doubles the delay"
        );
        assert!(second.next_attempt_at <= after_second + 4);
    }

    #[tokio::test]
    async fn fail_truncates_the_error_text() {
        let (provider, inbox) = direct_env(6).await;
        inbox.record(event("evt_1")).await.expect("record");
        let id = inbox.claim(unix_now_secs(), 10, 30).await.expect("claim")[0]
            .id
            .clone();

        let long_error = "x".repeat(MAX_ERROR_CHARS + 100);
        inbox.fail(&id, &long_error, 1).await.expect("fail");
        let stored = row(&provider, &id).await;
        assert_eq!(
            stored.last_error.expect("error stored").chars().count(),
            MAX_ERROR_CHARS
        );
    }

    #[tokio::test]
    async fn purge_processed_removes_only_rows_before_the_cutoff() {
        let (provider, inbox) = direct_env(3).await;
        inbox.record(event("evt_old")).await.expect("record old");
        inbox.record(event("evt_new")).await.expect("record new");
        let claimed = inbox.claim(unix_now_secs(), 10, 30).await.expect("claim");
        assert_eq!(claimed.len(), 2);
        for entry in &claimed {
            inbox.complete(&entry.id).await.expect("complete");
        }

        // Test-only fixture: back-date one processed row so the cutoff is
        // deterministic (the port itself always stamps "now").
        let stored = rows(&provider).await;
        let old = stored
            .iter()
            .find(|row| row.payload.contains("evt_old"))
            .expect("old row");
        let old_id = old.id.clone();
        let scope = provider.scope();
        let mut ctx: DbContext = scope.get_owned().expect("context");
        let query_id = old_id.clone();
        linq!(
            ctx.set::<PaymentWebhookInbox>(),
            |row: PaymentWebhookInbox| row.id == query_id
        )
        .execute_update()
        .set_column(PaymentWebhookInbox::COLUMN_PROCESSED_AT, Some(1_000i64))
        .execute()
        .await
        .expect("back-date processed_at");
        drop(ctx);

        assert_eq!(inbox.purge_processed(2_000).await.expect("purge"), 1);
        let remaining = rows(&provider).await;
        assert_eq!(remaining.len(), 1, "recent processed row is kept");
        assert!(remaining[0].payload.contains("evt_new"));
        assert_eq!(inbox.purge_processed(2_000).await.expect("purge again"), 0);
    }
}
