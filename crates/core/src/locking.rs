//! Lease-based advisory locks for cross-instance coordination.
//!
//! A lease is a time-bounded ownership token: [`IAdvisoryLock::try_acquire`]
//! returns `None` while another owner holds the key, and an **expired** lease
//! is reclaimable even when its owner crashed. Release and renew require the
//! token, so a caller that lost its lease cannot delete someone else's.
//!
//! # Deployment semantics
//!
//! * **Single process**: `webx_host::advisory_lock::InMemoryAdvisoryLock` is
//!   enough for tests and single-node deployments; it does not coordinate with
//!   other processes.
//! * **Multiple instances**: use a shared backend — `rust-webx-redis` ships
//!   `RedisAdvisoryLock` (SET NX PX + Lua compare-and-delete/renew); a database
//!   implementation (PostgreSQL advisory locks, SQLite single-writer) belongs to
//!   the ORM/starter side and implements the same trait.
//! * **Containers**: leases expire, so a killed container cannot deadlock its
//!   peers; keep the TTL comfortably above the critical section.
//! * **Performance**: acquire/release are one backend round trip. Never hold a
//!   lease while waiting on user input; `renew` for long-running work.
//!
//! ```ignore
//! // Quota check + decrement, serialized across instances.
//! if let Some(lease) = lock.try_acquire("quota:tenant-1", Duration::from_secs(5)).await? {
//!     let remaining = load_and_decrement().await?;
//!     lock.release(&lease).await?;
//! }
//! ```
//!
//! When a short wait is acceptable, call [`IAdvisoryLock::acquire_with_timeout`]
//! instead of writing a retry loop: the framework owns the backoff policy
//! (10 ms, doubling, capped at 200 ms) and returns `Ok(None)` at the deadline.

use std::time::Duration;

use async_trait::async_trait;

use crate::error::Result;

/// First retry delay used by [`IAdvisoryLock::acquire_with_timeout`].
const ACQUIRE_RETRY_BASE: Duration = Duration::from_millis(10);

/// Upper bound for one retry delay in [`IAdvisoryLock::acquire_with_timeout`].
const ACQUIRE_RETRY_MAX: Duration = Duration::from_millis(200);

/// A held advisory lock lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvisoryLease {
    /// Key that is locked (already namespaced by the caller).
    pub key: String,
    /// Ownership token; [`IAdvisoryLock::release`] and
    /// [`IAdvisoryLock::renew`] require it.
    pub token: String,
    /// Lease duration requested at acquisition (reused by `renew`).
    pub ttl: Duration,
}

/// Distributed advisory lock with lease semantics.
///
/// Implementations must return `Ok(None)` for plain contention and an error only
/// when the backend itself failed (a retryable 503), so callers can tell
/// "someone else holds it" from "the lock service is down".
#[async_trait]
pub trait IAdvisoryLock: Send + Sync {
    /// Try to acquire `key` for `ttl`; `None` means another owner holds it.
    async fn try_acquire(&self, key: &str, ttl: Duration) -> Result<Option<AdvisoryLease>>;

    /// Acquire `key` for `ttl`, retrying with backoff for at most `wait`.
    ///
    /// This is the framework-provided waiting policy: business code must not
    /// write its own `loop { try_acquire; sleep; }`. The method calls
    /// [`Self::try_acquire`] immediately, then retries while the key stays
    /// contended, starting at 10 ms per pause, doubling, capped at 200 ms; the
    /// last pause is clamped to the remaining wait, so the call returns around
    /// `wait` (plus one backend round trip).
    ///
    /// # Contract
    ///
    /// * `Ok(Some(lease))` — acquired, possibly immediately.
    /// * `Ok(None)` — still held by another owner when `wait` elapsed. Same
    ///   "contention, not an error" meaning as a single [`Self::try_acquire`]
    ///   attempt, so callers map it to their busy path (a `429`, a skip, …).
    /// * `Err(_)` — the backend itself failed; returned **unchanged and without
    ///   retrying**, because an unreachable lock service is not contention.
    /// * `wait == Duration::ZERO` is exactly one attempt. An invalid `ttl`
    ///   (a backend validation error) is likewise returned on the first call.
    ///
    /// # Cancellation
    ///
    /// The future holds no lock-related state: dropping it mid-wait simply stops
    /// retrying. A lease acquired just before the drop is not released
    /// automatically — it expires with its `ttl`, so keep the TTL comfortably
    /// above the critical section (same rule as [`Self::try_acquire`]).
    async fn acquire_with_timeout(
        &self,
        key: &str,
        ttl: Duration,
        wait: Duration,
    ) -> Result<Option<AdvisoryLease>> {
        let deadline = tokio::time::Instant::now() + wait;
        let mut pause = ACQUIRE_RETRY_BASE;
        loop {
            if let Some(lease) = self.try_acquire(key, ttl).await? {
                return Ok(Some(lease));
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let remaining = deadline - now;
            tokio::time::sleep(pause.min(remaining)).await;
            pause = pause.saturating_mul(2).min(ACQUIRE_RETRY_MAX);
        }
    }

    /// Release a lease. Returns `false` when it expired or was re-acquired.
    async fn release(&self, lease: &AdvisoryLease) -> Result<bool>;

    /// Extend a held lease by its original TTL. Returns `false` when it was lost.
    async fn renew(&self, lease: &AdvisoryLease) -> Result<bool>;
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, PoisonError};

    use super::*;
    use crate::error::Error;

    /// In-memory lock used to exercise the default waiting policy.
    ///
    /// It counts `try_acquire` calls so a test can prove "one attempt" and
    /// "no retry after a backend error".
    struct TestLock {
        held: Mutex<Option<AdvisoryLease>>,
        attempts: AtomicUsize,
        /// When set, every attempt fails with this status instead of contending.
        backend_status: Option<u16>,
    }

    impl TestLock {
        fn new() -> Self {
            Self {
                held: Mutex::new(None),
                attempts: AtomicUsize::new(0),
                backend_status: None,
            }
        }

        fn failing(status: u16) -> Self {
            Self {
                backend_status: Some(status),
                ..Self::new()
            }
        }

        fn held(&self) -> std::sync::MutexGuard<'_, Option<AdvisoryLease>> {
            self.held.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    #[async_trait]
    impl IAdvisoryLock for TestLock {
        async fn try_acquire(&self, key: &str, ttl: Duration) -> Result<Option<AdvisoryLease>> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            if ttl.is_zero() {
                return Err(Error::Validation(
                    "advisory lock ttl must be positive".to_string(),
                ));
            }
            if let Some(status) = self.backend_status {
                return Err(Error::Status(status, "lock backend down".to_string()));
            }
            let mut held = self.held();
            if held.is_some() {
                return Ok(None);
            }
            let lease = AdvisoryLease {
                key: key.to_string(),
                token: "token".to_string(),
                ttl,
            };
            *held = Some(lease.clone());
            Ok(Some(lease))
        }

        async fn release(&self, _lease: &AdvisoryLease) -> Result<bool> {
            Ok(self.held().take().is_some())
        }

        async fn renew(&self, _lease: &AdvisoryLease) -> Result<bool> {
            Ok(true)
        }
    }

    #[test]
    fn lease_keeps_key_token_and_ttl() {
        let lease = AdvisoryLease {
            key: "quota:tenant-1".to_string(),
            token: "t".to_string(),
            ttl: Duration::from_secs(5),
        };
        assert_eq!(lease.key, "quota:tenant-1");
        assert_eq!(lease.ttl, Duration::from_secs(5));
    }

    #[tokio::test]
    async fn acquire_with_timeout_returns_none_at_the_deadline() {
        let lock = TestLock::new();
        let _busy = lock
            .try_acquire("quota:t1", Duration::from_secs(30))
            .await
            .expect("acquire")
            .expect("free");

        let started = tokio::time::Instant::now();
        let outcome = lock
            .acquire_with_timeout(
                "quota:t1",
                Duration::from_secs(30),
                Duration::from_millis(60),
            )
            .await
            .expect("contention is not an error");
        let elapsed = started.elapsed();

        assert!(outcome.is_none(), "the key stays held for the whole wait");
        assert!(
            elapsed >= Duration::from_millis(60),
            "must wait for the full window, elapsed {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "must stop at the deadline, elapsed {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn acquire_with_timeout_gets_the_lease_when_released_during_the_wait() {
        let lock = Arc::new(TestLock::new());
        let lease = lock
            .try_acquire("job:daily", Duration::from_secs(30))
            .await
            .expect("acquire")
            .expect("free");

        let releaser = Arc::clone(&lock);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            releaser.release(&lease).await.expect("release");
        });

        let acquired = lock
            .acquire_with_timeout("job:daily", Duration::from_secs(5), Duration::from_secs(2))
            .await
            .expect("acquire")
            .expect("the release frees the key inside the window");
        assert_eq!(acquired.key, "job:daily");
    }

    #[tokio::test]
    async fn acquire_with_timeout_zero_wait_is_a_single_attempt() {
        let lock = TestLock::new();
        let _busy = lock
            .try_acquire("quota:t1", Duration::from_secs(30))
            .await
            .expect("acquire")
            .expect("free");
        lock.attempts.store(0, Ordering::SeqCst);

        assert!(lock
            .acquire_with_timeout("quota:t1", Duration::from_secs(30), Duration::ZERO)
            .await
            .expect("contention")
            .is_none());
        assert_eq!(
            lock.attempts.load(Ordering::SeqCst),
            1,
            "a zero wait must not sleep or retry"
        );
    }

    #[tokio::test]
    async fn acquire_with_timeout_propagates_a_backend_error_without_retrying() {
        let lock = TestLock::failing(503);
        let error = lock
            .acquire_with_timeout("k", Duration::from_secs(5), Duration::from_secs(1))
            .await
            .expect_err("the backend error is returned as-is");
        assert_eq!(error.status_code(), 503);
        assert_eq!(
            lock.attempts.load(Ordering::SeqCst),
            1,
            "a failing backend must not be retried by the waiting policy"
        );
    }

    #[tokio::test]
    async fn acquire_with_timeout_propagates_an_invalid_ttl_without_waiting() {
        let lock = TestLock::new();
        let started = tokio::time::Instant::now();

        let error = lock
            .acquire_with_timeout("k", Duration::ZERO, Duration::from_secs(1))
            .await
            .expect_err("the backend rejects the ttl");

        assert_eq!(error.status_code(), 400);
        assert_eq!(lock.attempts.load(Ordering::SeqCst), 1);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "an Err must return immediately, not after the wait window"
        );
    }
}
