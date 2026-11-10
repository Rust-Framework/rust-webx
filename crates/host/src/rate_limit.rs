//! Per-IP token-bucket rate-limiting middleware.
//!
//! Configure via `appsettings.json` `RateLimit` section or manually:
//!
//! ```ignore
//! .use_middleware_with(|| {
//!     Arc::new(RateLimitMiddleware::from_config(&options.rate_limit))
//! })
//! ```

use crate::problem_response::write_problem;
use std::collections::HashMap;
use std::net::IpAddr;
use std::ops::ControlFlow;
use std::str::FromStr;
use std::time::Instant;
use tokio::sync::Mutex;
use webx_core::config::RateLimitSection;
use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;

struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(burst_size: f64) -> Self {
        Self {
            tokens: burst_size,
            last_refill: Instant::now(),
        }
    }

    fn try_consume(&mut self, rate: f64, burst: f64) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * rate).min(burst);
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Inner rate-limiting state shared behind a [`Mutex`].
pub struct RateLimiter {
    buckets: Mutex<HashMap<IpAddr, TokenBucket>>,
    rate: f64,
    burst: f64,
    max_ips: usize,
    trust_proxy: bool,
}

impl RateLimiter {
    pub fn new(requests_per_second: f64, burst_size: u32, max_tracked_ips: usize) -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            rate: requests_per_second,
            burst: burst_size as f64,
            max_ips: max_tracked_ips.max(1),
            trust_proxy: false,
        }
    }

    pub fn with_trust_proxy(mut self, trust: bool) -> Self {
        self.trust_proxy = trust;
        self
    }

    async fn allow(&self, ip: IpAddr) -> bool {
        let mut buckets = self.buckets.lock().await;
        if !buckets.contains_key(&ip) && buckets.len() >= self.max_ips {
            evict_oldest(&mut buckets);
        }
        let bucket = buckets
            .entry(ip)
            .or_insert_with(|| TokenBucket::new(self.burst));
        bucket.try_consume(self.rate, self.burst)
    }

    /// Current number of tracked client IPs (for tests).
    pub async fn tracked_ip_count(&self) -> usize {
        self.buckets.lock().await.len()
    }

    /// Seconds a client should wait before retrying (RFC 6585 `Retry-After`).
    ///
    /// A bucket refills one token every `1 / rate` seconds; a whole second is
    /// the smallest useful hint, and 1s is used when the rate is unset.
    pub fn retry_after_secs(&self) -> u64 {
        if self.rate <= 0.0 {
            return 1;
        }
        ((1.0 / self.rate).ceil() as u64).max(1)
    }
}

fn evict_oldest(buckets: &mut HashMap<IpAddr, TokenBucket>) {
    if buckets.is_empty() {
        return;
    }
    let oldest = buckets
        .iter()
        .min_by_key(|(_, b)| b.last_refill)
        .map(|(ip, _)| *ip);
    if let Some(ip) = oldest {
        buckets.remove(&ip);
    }
}

pub struct RateLimitMiddleware {
    limiter: RateLimiter,
    trust_proxy: bool,
}

impl RateLimitMiddleware {
    pub fn new(requests_per_second: f64, burst_size: u32) -> Self {
        Self::new_with_max_ips(requests_per_second, burst_size, 10_000)
    }

    pub fn new_with_max_ips(
        requests_per_second: f64,
        burst_size: u32,
        max_tracked_ips: usize,
    ) -> Self {
        Self {
            limiter: RateLimiter::new(requests_per_second, burst_size, max_tracked_ips),
            trust_proxy: false,
        }
    }

    pub fn from_config(cfg: &RateLimitSection) -> Self {
        Self {
            limiter: RateLimiter::new(cfg.requests_per_second, cfg.burst_size, cfg.max_tracked_ips),
            trust_proxy: cfg.trust_proxy,
        }
    }
}

#[async_trait::async_trait]
impl IMiddleware for RateLimitMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        // No peer address means the host cannot tell clients apart. Falling
        // back to a constant would turn per-client limiting into one global
        // bucket, so the limiter passes the request through and the host is
        // expected to expose `IHttpRequest::remote_addr`.
        let Some(ip) = extract_client_ip(ctx, self.trust_proxy) else {
            tracing::debug!("rate limit skipped: request has no peer address");
            return Ok(ControlFlow::Continue(()));
        };

        if !self.limiter.allow(ip).await {
            let retry_after = self.limiter.retry_after_secs().to_string();
            ctx.response_mut().set_header("retry-after", &retry_after);
            write_problem(ctx, 429, "Too Many Requests").await;
            return Ok(ControlFlow::Break(()));
        }

        Ok(ControlFlow::Continue(()))
    }
}

/// Resolve the client IP the token bucket is keyed by.
///
/// The connection's peer address is authoritative. `trust_proxy` additionally
/// accepts `X-Forwarded-For` / `X-Real-IP`, which is only sound behind a proxy
/// that overwrites those headers; a deployment exposed directly to clients
/// must keep it `false` (a client can otherwise rotate the header to bypass
/// the limiter).
fn extract_client_ip(ctx: &dyn IHttpContext, trust_proxy: bool) -> Option<IpAddr> {
    if trust_proxy {
        if let Some(fwd) = ctx.request().header("x-forwarded-for") {
            let first = fwd.split(',').next().unwrap_or("").trim();
            if let Ok(ip) = IpAddr::from_str(first) {
                return Some(ip);
            }
        }

        if let Some(real) = ctx.request().header("x-real-ip") {
            if let Ok(ip) = IpAddr::from_str(real.trim()) {
                return Some(ip);
            }
        }
    }

    ctx.request().remote_addr().map(|addr| addr.ip())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[tokio::test]
    async fn rate_limiter_evicts_oldest_when_max_ips_reached() {
        let limiter = RateLimiter::new(100.0, 10, 2);
        assert!(limiter.allow(IpAddr::V4(Ipv4Addr::new(1, 0, 0, 1))).await);
        assert!(limiter.allow(IpAddr::V4(Ipv4Addr::new(2, 0, 0, 1))).await);
        assert_eq!(limiter.tracked_ip_count().await, 2);

        assert!(limiter.allow(IpAddr::V4(Ipv4Addr::new(3, 0, 0, 1))).await);
        assert_eq!(limiter.tracked_ip_count().await, 2);
    }

    #[tokio::test]
    async fn retry_after_matches_the_refill_rate() {
        // 10 requests/s: a token every 0.1s, so the smallest whole-second hint
        // is still 1s.
        let fast = RateLimiter::new(10.0, 1, 10);
        assert_eq!(fast.retry_after_secs(), 1);

        // 0.5 requests/s: one token every 2s.
        let slow = RateLimiter::new(0.5, 1, 10);
        assert_eq!(slow.retry_after_secs(), 2);

        // Never report 0 (that would invite an immediate retry storm).
        let unset = RateLimiter::new(0.0, 1, 10);
        assert_eq!(unset.retry_after_secs(), 1);
    }
}
