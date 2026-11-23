//! Process-local advisory lock (tests and single-node deployments).
//!
//! [`InMemoryAdvisoryLock`] implements
//! [`IAdvisoryLock`] with the same lease
//! semantics as the distributed backends: `try_acquire` returns `None` under
//! contention, expired leases are reclaimable, and release/renew require the
//! ownership token.
//!
//! It is **not** a distributed lock: two processes each hold their own map, so
//! use `rust-webx-redis`'s `RedisAdvisoryLock` (or a database implementation)
//! when more than one instance can take the same key.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use uuid::Uuid;

use webx_core::error::{Error, Result};
use webx_core::locking::{AdvisoryLease, IAdvisoryLock};

struct Held {
    token: String,
    expires_at: Instant,
}

/// Process-local lease lock.
pub struct InMemoryAdvisoryLock {
    held: Mutex<HashMap<String, Held>>,
}

impl Default for InMemoryAdvisoryLock {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryAdvisoryLock {
    /// An empty lock.
    pub fn new() -> Self {
        Self {
            held: Mutex::new(HashMap::new()),
        }
    }

    /// Number of **live** leases (expired entries are pruned).
    pub fn held_count(&self) -> usize {
        let now = Instant::now();
        let mut held = self.lock();
        held.retain(|_, entry| entry.expires_at > now);
        held.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Held>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn validate_ttl(ttl: Duration) -> Result<()> {
    if ttl.is_zero() {
        return Err(Error::Validation(
            "advisory lock ttl must be positive".to_string(),
        ));
    }
    Ok(())
}

#[async_trait]
impl IAdvisoryLock for InMemoryAdvisoryLock {
    async fn try_acquire(&self, key: &str, ttl: Duration) -> Result<Option<AdvisoryLease>> {
        validate_ttl(ttl)?;
        let now = Instant::now();
        let mut held = self.lock();
        if held.get(key).is_some_and(|entry| entry.expires_at > now) {
            return Ok(None);
        }
        let token = Uuid::new_v4().to_string();
        held.insert(
            key.to_string(),
            Held {
                token: token.clone(),
                expires_at: now + ttl,
            },
        );
        Ok(Some(AdvisoryLease {
            key: key.to_string(),
            token,
            ttl,
        }))
    }

    async fn release(&self, lease: &AdvisoryLease) -> Result<bool> {
        let now = Instant::now();
        let mut held = self.lock();
        let owned = held
            .get(&lease.key)
            .is_some_and(|entry| entry.token == lease.token && entry.expires_at > now);
        if owned {
            held.remove(&lease.key);
        }
        Ok(owned)
    }

    async fn renew(&self, lease: &AdvisoryLease) -> Result<bool> {
        let now = Instant::now();
        let mut held = self.lock();
        let expired = match held.get(&lease.key) {
            Some(entry) if entry.token == lease.token => entry.expires_at <= now,
            // Another owner (or nothing) holds the key: never touch it.
            _ => return Ok(false),
        };
        if expired {
            held.remove(&lease.key);
            return Ok(false);
        }
        if let Some(entry) = held.get_mut(&lease.key) {
            entry.expires_at = now + lease.ttl;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn contention_is_reported_as_none_and_release_frees_the_key() {
        let lock = InMemoryAdvisoryLock::new();
        let first = lock
            .try_acquire("quota:t1", Duration::from_secs(30))
            .await
            .expect("acquire")
            .expect("free key");
        assert_eq!(lock.held_count(), 1);
        assert!(lock
            .try_acquire("quota:t1", Duration::from_secs(30))
            .await
            .expect("contention is not an error")
            .is_none());

        assert!(lock.release(&first).await.expect("release"));
        assert_eq!(lock.held_count(), 0);
        assert!(lock
            .try_acquire("quota:t1", Duration::from_secs(30))
            .await
            .expect("acquire")
            .is_some());
    }

    #[tokio::test]
    async fn an_expired_lease_is_reclaimable() {
        let lock = InMemoryAdvisoryLock::new();
        let stale = lock
            .try_acquire("k", Duration::from_millis(10))
            .await
            .expect("acquire")
            .expect("free");
        tokio::time::sleep(Duration::from_millis(30)).await;

        let fresh = lock
            .try_acquire("k", Duration::from_secs(30))
            .await
            .expect("acquire")
            .expect("expired lease is reclaimable");
        assert_ne!(fresh.token, stale.token);

        // The stale owner can neither release nor renew the new lease.
        assert!(!lock.release(&stale).await.expect("release"));
        assert!(!lock.renew(&stale).await.expect("renew"));
        assert!(lock.held_count() == 1);
    }

    #[tokio::test]
    async fn renew_extends_a_live_lease_and_rejects_a_lost_one() {
        let lock = InMemoryAdvisoryLock::new();
        let lease = lock
            .try_acquire("k", Duration::from_millis(40))
            .await
            .expect("acquire")
            .expect("free");
        assert!(lock.renew(&lease).await.expect("renew"));

        // A forged token cannot renew or release.
        let forged = AdvisoryLease {
            key: lease.key.clone(),
            token: "forged".to_string(),
            ttl: lease.ttl,
        };
        assert!(!lock.renew(&forged).await.expect("renew"));
        assert!(!lock.release(&forged).await.expect("release"));
    }

    #[tokio::test]
    async fn zero_ttl_is_rejected() {
        let lock = InMemoryAdvisoryLock::new();
        let error = lock
            .try_acquire("k", Duration::ZERO)
            .await
            .expect_err("zero ttl");
        assert_eq!(error.status_code(), 400);
    }
}
