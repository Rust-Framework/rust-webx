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
use rust_webx_core::config::RateLimitSection;
use rust_webx_core::error::Result;
use rust_webx_core::http::IHttpContext;
use rust_webx_core::middleware::IMiddleware;
use std::collections::HashMap;
use std::net::IpAddr;
use std::ops::ControlFlow;
use std::str::FromStr;
use std::time::Instant;
use tokio::sync::Mutex;

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

    pub fn new_with_max_ips(requests_per_second: f64, burst_size: u32, max_tracked_ips: usize) -> Self {
        Self {
            limiter: RateLimiter::new(requests_per_second, burst_size, max_tracked_ips),
            trust_proxy: false,
        }
    }

    pub fn from_config(cfg: &RateLimitSection) -> Self {
        Self {
            limiter: RateLimiter::new(
                cfg.requests_per_second,
                cfg.burst_size,
                cfg.max_tracked_ips,
            ),
            trust_proxy: cfg.trust_proxy,
        }
    }
}

#[async_trait::async_trait]
impl IMiddleware for RateLimitMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        let ip = extract_client_ip(ctx, self.trust_proxy);

        if !self.limiter.allow(ip).await {
            write_problem(ctx, 429, "Too Many Requests").await;
            return Ok(ControlFlow::Break(()));
        }

        Ok(ControlFlow::Continue(()))
    }
}

fn extract_client_ip(ctx: &dyn IHttpContext, trust_proxy: bool) -> IpAddr {
    if trust_proxy {
        if let Some(fwd) = ctx.request().header("x-forwarded-for") {
            let first = fwd.split(',').next().unwrap_or("").trim();
            if let Ok(ip) = IpAddr::from_str(first) {
                return ip;
            }
        }

        if let Some(real) = ctx.request().header("x-real-ip") {
            if let Ok(ip) = IpAddr::from_str(real.trim()) {
                return ip;
            }
        }
    }

    IpAddr::from_str("127.0.0.1").unwrap()
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
}
