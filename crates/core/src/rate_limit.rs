//! Rate-limiting port.
//!
//! The framework owns the middleware contract (`429` + `Retry-After` +
//! `problem+json`); the counter store is a port so a distributed deployment can
//! back it with Redis or another shared store without changing handlers.
//!
//! [`IRateLimiter`] is called once per request with an opaque key — the client
//! IP by default. Implementations must be cheap and bounded: the hot path may
//! not block on I/O outside their own store.

/// Outcome of one rate-limit check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitDecision {
    /// Whether the request may proceed.
    pub allowed: bool,
    /// Whole seconds the client should wait before retrying (RFC 6585).
    ///
    /// Always at least `1` when `allowed` is `false`; reporting `0` would invite
    /// an immediate retry storm.
    pub retry_after_secs: u64,
}

impl RateLimitDecision {
    /// Allow the request.
    pub const fn allow() -> Self {
        Self {
            allowed: true,
            retry_after_secs: 1,
        }
    }

    /// Deny the request, telling the client when to retry.
    pub const fn deny(retry_after_secs: u64) -> Self {
        Self {
            allowed: false,
            retry_after_secs: if retry_after_secs == 0 {
                1
            } else {
                retry_after_secs
            },
        }
    }
}

/// Distributed-capable rate-limit store.
///
/// The default implementation is an in-process, sharded token bucket
/// (`webx_host::rate_limit::InMemoryRateLimiter`). A multi-instance deployment
/// that needs a cluster-wide limit implements this port over a shared store.
#[async_trait::async_trait]
pub trait IRateLimiter: Send + Sync {
    /// Consume one token for `key` and report whether the request may proceed.
    async fn check(&self, key: &str) -> RateLimitDecision;
}
