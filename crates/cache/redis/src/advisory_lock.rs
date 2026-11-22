//! Redis-backed advisory lock.
//!
//! [`RedisAdvisoryLock`] implements
//! [`IAdvisoryLock`](webx_core::locking::IAdvisoryLock) with `SET NX PX` plus
//! compare-and-delete / compare-and-extend Lua scripts:
//!
//! * acquire: `SET <prefix>lock:<key> <token> NX PX <ttl>`;
//! * release: delete only when the stored token matches (a stale owner cannot
//!   release a lease that was re-acquired after expiry);
//! * renew: `PEXPIRE` only when the stored token matches.
//!
//! The connection is the same lazily connected [`RedisConnection`] used by the
//! cache/event transport, so a Redis outage surfaces as a retryable 503
//! (`transport_error`) rather than a panic.
//!
//! ```no_run
//! use std::sync::Arc;
//! use std::time::Duration;
//! use webx_core::locking::IAdvisoryLock;
//! use webx_redis::{RedisAdvisoryLock, RedisConnection};
//!
//! # async fn demo() -> webx_core::error::Result<()> {
//! let connection = Arc::new(RedisConnection::new(
//!     "redis://127.0.0.1:6379",
//!     Duration::from_secs(3),
//!     Duration::from_secs(2),
//! ));
//! let lock = RedisAdvisoryLock::new(connection, "app:");
//! if let Some(lease) = lock.try_acquire("quota:t1", Duration::from_secs(5)).await? {
//!     // critical section
//!     lock.release(&lease).await?;
//! }
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use uuid::Uuid;
use webx_core::error::{Error, Result};
use webx_core::locking::{AdvisoryLease, IAdvisoryLock};

use crate::connection::{transport_error, RedisConnection};

/// Delete the key only when the stored token matches.
const RELEASE_SCRIPT: &str = r#"
if redis.call('GET', KEYS[1]) == ARGV[1] then
    return redis.call('DEL', KEYS[1])
end
return 0
"#;

/// Extend the key only when the stored token matches.
const RENEW_SCRIPT: &str = r#"
if redis.call('GET', KEYS[1]) == ARGV[1] then
    return redis.call('PEXPIRE', KEYS[1], ARGV[2])
end
return 0
"#;

/// Redis advisory lock over a shared [`RedisConnection`].
pub struct RedisAdvisoryLock {
    connection: Arc<RedisConnection>,
    key_prefix: String,
}

impl RedisAdvisoryLock {
    /// Build a lock over `connection`; every key becomes
    /// `{key_prefix}lock:{key}` (use a non-empty prefix so several services can
    /// share one Redis without colliding).
    pub fn new(connection: Arc<RedisConnection>, key_prefix: impl Into<String>) -> Self {
        Self {
            connection,
            key_prefix: key_prefix.into(),
        }
    }

    /// Prefix applied to every lock key.
    pub fn key_prefix(&self) -> &str {
        &self.key_prefix
    }

    fn redis_key(&self, key: &str) -> String {
        if self.key_prefix.is_empty() {
            format!("lock:{key}")
        } else {
            format!("{}lock:{key}", self.key_prefix)
        }
    }
}

fn ttl_millis(ttl: Duration) -> Result<u64> {
    if ttl.is_zero() {
        return Err(Error::Validation(
            "advisory lock ttl must be positive".to_string(),
        ));
    }
    let millis = ttl.as_millis();
    if millis == 0 {
        return Err(Error::Validation(
            "advisory lock ttl must be at least 1 ms".to_string(),
        ));
    }
    Ok(u64::try_from(millis).unwrap_or(u64::MAX))
}

#[async_trait]
impl IAdvisoryLock for RedisAdvisoryLock {
    async fn try_acquire(&self, key: &str, ttl: Duration) -> Result<Option<AdvisoryLease>> {
        let ttl_ms = ttl_millis(ttl)?;
        let mut connection = self
            .connection
            .manager()
            .await
            .map_err(|error| transport_error("advisory lock connection failed", error))?;
        let token = Uuid::new_v4().to_string();
        let redis_key = self.redis_key(key);
        let acquired: Option<String> = redis::cmd("SET")
            .arg(&redis_key)
            .arg(&token)
            .arg("NX")
            .arg("PX")
            .arg(ttl_ms)
            .query_async(&mut connection)
            .await
            .map_err(|error| transport_error("advisory lock acquire failed", error))?;
        Ok(acquired.map(|_| AdvisoryLease {
            key: key.to_string(),
            token,
            ttl,
        }))
    }

    async fn release(&self, lease: &AdvisoryLease) -> Result<bool> {
        let mut connection = self
            .connection
            .manager()
            .await
            .map_err(|error| transport_error("advisory lock connection failed", error))?;
        let released: i64 = redis::cmd("EVAL")
            .arg(RELEASE_SCRIPT)
            .arg(1)
            .arg(self.redis_key(&lease.key))
            .arg(&lease.token)
            .query_async(&mut connection)
            .await
            .map_err(|error| transport_error("advisory lock release failed", error))?;
        Ok(released == 1)
    }

    async fn renew(&self, lease: &AdvisoryLease) -> Result<bool> {
        let ttl_ms = ttl_millis(lease.ttl)?;
        let mut connection = self
            .connection
            .manager()
            .await
            .map_err(|error| transport_error("advisory lock connection failed", error))?;
        let renewed: i64 = redis::cmd("EVAL")
            .arg(RENEW_SCRIPT)
            .arg(1)
            .arg(self.redis_key(&lease.key))
            .arg(&lease.token)
            .arg(ttl_ms)
            .query_async(&mut connection)
            .await
            .map_err(|error| transport_error("advisory lock renew failed", error))?;
        Ok(renewed == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_keys_are_namespaced_and_ttl_is_validated() {
        let connection = Arc::new(RedisConnection::new(
            "redis://127.0.0.1:6379",
            Duration::from_secs(1),
            Duration::from_secs(1),
        ));
        let lock = RedisAdvisoryLock::new(connection, "app:");
        assert_eq!(lock.key_prefix(), "app:");
        assert_eq!(lock.redis_key("quota:t1"), "app:lock:quota:t1");

        let unprefixed = RedisAdvisoryLock::new(
            Arc::new(RedisConnection::new(
                "redis://127.0.0.1:6379",
                Duration::from_secs(1),
                Duration::from_secs(1),
            )),
            "",
        );
        assert_eq!(unprefixed.redis_key("k"), "lock:k");

        assert!(ttl_millis(Duration::ZERO).is_err());
        assert_eq!(ttl_millis(Duration::from_millis(5)).expect("ttl"), 5);
    }
}
