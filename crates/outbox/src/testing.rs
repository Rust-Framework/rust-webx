//! Behavioural contract shared by every [`IOutbox`] implementation.
//!
//! Storage adapters differ in where rows live, never in what the dispatcher
//! relies on. This module runs the same cases against the built-in
//! [`InMemoryOutbox`](crate::InMemoryOutbox) and against database-backed stores
//! (their test suites call it with their own seeder), so a regression in lease,
//! backoff, dead-letter or purge semantics is caught in one place.

use std::sync::Arc;

use async_trait::async_trait;

use crate::port::IOutbox;
use crate::time::unix_now_secs;

/// Fixture used by [`run_contract`].
///
/// Each contract case gets a fresh seeder, so a store that keeps state between
/// cases (the in-memory store) is isolated by construction.
#[async_trait]
pub trait OutboxContractSeeder: Send + Sync {
    /// Create the schema/connection state the case needs (no-op for in-memory).
    async fn prepare(&self);

    /// The store under test, sharing state with [`Self::seed`].
    fn store(&self) -> Arc<dyn IOutbox>;

    /// Insert one pending row that is due immediately, using the adapter's own
    /// producer path (its transaction/context helper, not the worker port).
    async fn seed(&self, id: &str, event_name: &str, payload: &str);
}

/// Run the full [`IOutbox`] contract.
///
/// `make` builds a fresh, isolated fixture per case; `max_attempts` is the
/// dead-letter threshold the seeded store was configured with.
pub async fn run_contract<S: OutboxContractSeeder>(make: impl Fn() -> S, max_attempts: i32) {
    assert!(max_attempts >= 1, "max_attempts is at least 1");

    {
        let seeder = make();
        seeder.prepare().await;
        claim_is_leased(&seeder).await;
    }
    {
        let seeder = make();
        seeder.prepare().await;
        complete_retires(&seeder).await;
    }
    {
        let seeder = make();
        seeder.prepare().await;
        fail_backs_off_then_dead_letters(&seeder, max_attempts).await;
    }
    {
        let seeder = make();
        seeder.prepare().await;
        dead_letter_retires(&seeder).await;
    }
    {
        let seeder = make();
        seeder.prepare().await;
        purge_removes_only_finished_rows(&seeder).await;
    }
}

/// A claimed row is invisible to other dispatchers until its lease expires.
async fn claim_is_leased<S: OutboxContractSeeder>(seeder: &S) {
    seeder.seed("lease-1", "contract.event", "{\"n\":1}").await;
    let store = seeder.store();
    let now = unix_now_secs();

    let claimed = store.claim(now, 10, 60).await.expect("claim");
    assert_eq!(claimed.len(), 1, "a due row must be claimable");
    let entry = &claimed[0];
    assert_eq!(entry.id, "lease-1");
    assert_eq!(entry.event_name, "contract.event");
    assert_eq!(entry.payload, "{\"n\":1}");
    assert_eq!(entry.attempts, 0);

    assert!(
        store.claim(now, 10, 60).await.expect("claim").is_empty(),
        "a leased row must not be claimed twice"
    );
    assert!(
        store
            .claim(now + 59, 10, 60)
            .await
            .expect("claim")
            .is_empty(),
        "a lease must hold until it expires"
    );
    let reclaimed = store.claim(now + 60, 10, 60).await.expect("claim");
    assert_eq!(
        reclaimed.len(),
        1,
        "an expired lease must let another dispatcher reclaim the row"
    );
    assert_eq!(reclaimed[0].id, "lease-1");
    assert_eq!(reclaimed[0].attempts, 0);
}

/// `complete` is terminal and idempotent; unknown ids are a no-op.
async fn complete_retires<S: OutboxContractSeeder>(seeder: &S) {
    seeder.seed("complete-1", "contract.event", "{}").await;
    let store = seeder.store();
    let now = unix_now_secs();
    assert_eq!(store.claim(now, 10, 60).await.expect("claim").len(), 1);

    store.complete("complete-1").await.expect("complete");
    assert!(
        store
            .claim(unix_now_secs() + 86_400, 10, 60)
            .await
            .expect("claim")
            .is_empty(),
        "a delivered row must not be claimed again"
    );
    store
        .complete("complete-1")
        .await
        .expect("re-completing is a no-op");
    store
        .complete("missing")
        .await
        .expect("completing an unknown id is a no-op");
}

/// `fail` backs off before the next attempt and dead-letters at the threshold.
async fn fail_backs_off_then_dead_letters<S: OutboxContractSeeder>(seeder: &S, max_attempts: i32) {
    seeder.seed("fail-1", "contract.event", "{}").await;
    let store = seeder.store();
    let base = 2;

    let mut expected_attempts = 0;
    loop {
        // Backoff schedule: base, 2x, 4x, ... capped at 2^10 (documented in
        // `IOutbox::fail`); `+ 1s` covers the real clock advancing between calls.
        let delay = base * 2i64.pow((expected_attempts as u32).min(10));
        let due = unix_now_secs() + delay + 1;
        let claimed = store.claim(due, 10, 60).await.expect("claim");
        assert_eq!(
            claimed.len(),
            1,
            "the row must be claimable again after the backoff window"
        );
        assert_eq!(claimed[0].id, "fail-1");
        assert_eq!(
            claimed[0].attempts, expected_attempts,
            "each claim reports the failures so far"
        );

        store
            .fail("fail-1", "handler boom", base)
            .await
            .expect("fail");
        expected_attempts += 1;

        assert!(
            store
                .claim(unix_now_secs(), 10, 60)
                .await
                .expect("claim")
                .is_empty(),
            "a failure must back off instead of retrying immediately"
        );

        if expected_attempts >= max_attempts {
            break;
        }
    }

    assert_eq!(expected_attempts, max_attempts);
    assert!(
        store
            .claim(unix_now_secs() + 10 * 365 * 86_400, 10, 60)
            .await
            .expect("claim")
            .is_empty(),
        "a row dead-letters after max_attempts instead of retrying forever"
    );
}

/// `dead_letter` retires a row immediately and is idempotent.
async fn dead_letter_retires<S: OutboxContractSeeder>(seeder: &S) {
    seeder.seed("dead-1", "contract.event", "{}").await;
    let store = seeder.store();
    let now = unix_now_secs();
    assert_eq!(store.claim(now, 10, 60).await.expect("claim").len(), 1);

    store
        .dead_letter("dead-1", "no #[handler] registration")
        .await
        .expect("dead letter");
    assert!(
        store
            .claim(unix_now_secs() + 10 * 365 * 86_400, 10, 60)
            .await
            .expect("claim")
            .is_empty(),
        "a dead-lettered row must never be claimed again"
    );
    store
        .dead_letter("dead-1", "again")
        .await
        .expect("dead-lettering twice is a no-op");
}

/// `purge` deletes finished rows older than the cutoff and nothing else.
async fn purge_removes_only_finished_rows<S: OutboxContractSeeder>(seeder: &S) {
    seeder.seed("purge-delivered", "contract.event", "{}").await;
    seeder.seed("purge-dead", "contract.event", "{}").await;
    seeder.seed("purge-pending", "contract.event", "{}").await;
    let store = seeder.store();
    let now = unix_now_secs();

    let claimed = store.claim(now, 10, 60).await.expect("claim");
    assert_eq!(claimed.len(), 3);
    store.complete("purge-delivered").await.expect("complete");
    store
        .dead_letter("purge-dead", "boom")
        .await
        .expect("dead letter");

    let removed = store.purge(unix_now_secs() + 1).await.expect("purge");
    assert_eq!(removed, 2, "only finished rows are purged");

    let remaining = store
        .claim(unix_now_secs() + 61, 10, 60)
        .await
        .expect("claim");
    assert_eq!(remaining.len(), 1, "a pending row survives the purge");
    assert_eq!(remaining[0].id, "purge-pending");
    assert_eq!(store.purge(0).await.expect("purge"), 0);
}
