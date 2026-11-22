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

use std::time::Duration;

use async_trait::async_trait;

use crate::error::Result;

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

    /// Release a lease. Returns `false` when it expired or was re-acquired.
    async fn release(&self, lease: &AdvisoryLease) -> Result<bool>;

    /// Extend a held lease by its original TTL. Returns `false` when it was lost.
    async fn renew(&self, lease: &AdvisoryLease) -> Result<bool>;
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
