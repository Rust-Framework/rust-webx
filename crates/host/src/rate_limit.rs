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
    /// CIDR ranges whose peer addresses may supply `X-Forwarded-For`.
    trusted_proxies: Vec<IpCidr>,
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
            trusted_proxies: Vec::new(),
        }
    }

    pub fn from_config(cfg: &RateLimitSection) -> Self {
        let trusted_proxies: Vec<IpCidr> = cfg
            .trusted_proxies
            .iter()
            .filter_map(|value| match IpCidr::parse(value) {
                Some(cidr) => Some(cidr),
                None => {
                    tracing::warn!(
                        value,
                        "RateLimit.TrustedProxies entry is not a valid CIDR; ignoring it"
                    );
                    None
                }
            })
            .collect();
        if cfg.trust_proxy && trusted_proxies.is_empty() {
            tracing::warn!(
                "RateLimit.TrustProxy is enabled but TrustedProxies is empty; \
                 forwarded headers are ignored (a client could otherwise spoof them)"
            );
        }
        Self {
            limiter: RateLimiter::new(cfg.requests_per_second, cfg.burst_size, cfg.max_tracked_ips),
            trust_proxy: cfg.trust_proxy,
            trusted_proxies,
        }
    }
}

/// A parsed CIDR range used to identify trusted reverse proxies.
#[derive(Debug, Clone, Copy)]
struct IpCidr {
    network: IpAddr,
    prefix: u8,
}

impl IpCidr {
    fn parse(value: &str) -> Option<Self> {
        let (addr, prefix) = value.split_once('/')?;
        let network = IpAddr::from_str(addr.trim()).ok()?;
        let prefix: u8 = prefix.trim().parse().ok()?;
        let max_prefix = match network {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        (prefix <= max_prefix).then_some(Self { network, prefix })
    }

    fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, ip) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - self.prefix)
                };
                (u32::from(network) & mask) == (u32::from(ip) & mask)
            }
            (IpAddr::V6(network), IpAddr::V6(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - self.prefix)
                };
                (u128::from(network) & mask) == (u128::from(ip) & mask)
            }
            _ => false,
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
        let Some(ip) = extract_client_ip(ctx, self.trust_proxy, &self.trusted_proxies) else {
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
/// The connection's peer address is authoritative. When `trust_proxy` is on,
/// forwarded headers are honoured **only if the direct peer address belongs to
/// one of `trusted_proxies`**; the client is then the right-most
/// `X-Forwarded-For` entry that is not itself a trusted proxy (a client can
/// otherwise prepend arbitrary addresses to spoof the key).
fn extract_client_ip(
    ctx: &dyn IHttpContext,
    trust_proxy: bool,
    trusted_proxies: &[IpCidr],
) -> Option<IpAddr> {
    let peer = ctx.request().remote_addr().map(|addr| addr.ip());

    if trust_proxy && !trusted_proxies.is_empty() {
        let peer_is_trusted = peer.is_some_and(|ip| trusted_proxies.iter().any(|c| c.contains(ip)));
        if peer_is_trusted {
            if let Some(forwarded) = ctx.request().header("x-forwarded-for") {
                if let Some(client) = client_from_forwarded_for(forwarded, trusted_proxies) {
                    return Some(client);
                }
            }
            if let Some(real) = ctx.request().header("x-real-ip") {
                if let Ok(ip) = IpAddr::from_str(real.trim()) {
                    return Some(ip);
                }
            }
        }
    }

    peer
}

/// The right-most address in `X-Forwarded-For` that is not a trusted proxy.
///
/// Proxies append, so the right-most entries are the ones closest to the
/// server; skipping trusted proxies from the right yields the real client.
fn client_from_forwarded_for(value: &str, trusted_proxies: &[IpCidr]) -> Option<IpAddr> {
    let hops: Vec<IpAddr> = value
        .split(',')
        .filter_map(|part| IpAddr::from_str(part.trim()).ok())
        .collect();

    hops.iter()
        .rev()
        .find(|ip| !trusted_proxies.iter().any(|cidr| cidr.contains(**ip)))
        .copied()
        .or_else(|| hops.first().copied())
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

    #[test]
    fn cidr_matches_only_inside_the_range() {
        let cidr = IpCidr::parse("10.0.0.0/8").expect("valid cidr");
        assert!(cidr.contains("10.1.2.3".parse().unwrap()));
        assert!(!cidr.contains("11.0.0.1".parse().unwrap()));
        assert!(IpCidr::parse("0.0.0.0/0")
            .unwrap()
            .contains("203.0.113.9".parse().unwrap()));
        assert!(IpCidr::parse("not-a-cidr").is_none());
        assert!(IpCidr::parse("10.0.0.0/33").is_none());

        let v6 = IpCidr::parse("2001:db8::/32").expect("valid v6 cidr");
        assert!(v6.contains("2001:db8::1".parse().unwrap()));
        assert!(!v6.contains("2001:db9::1".parse().unwrap()));
    }

    #[test]
    fn forwarded_for_skips_trusted_proxies_from_the_right() {
        let proxies = vec![IpCidr::parse("10.0.0.0/8").unwrap()];
        assert_eq!(
            client_from_forwarded_for("203.0.113.7, 10.0.0.2, 10.0.0.3", &proxies),
            Some("203.0.113.7".parse().unwrap()),
            "the right-most untrusted hop is the client"
        );
        assert_eq!(
            client_from_forwarded_for("1.1.1.1, 203.0.113.7, 10.0.0.3", &proxies),
            Some("203.0.113.7".parse().unwrap()),
            "a spoofed left-most entry must not win"
        );
        assert_eq!(
            client_from_forwarded_for("10.0.0.1, 10.0.0.3", &proxies),
            Some("10.0.0.1".parse().unwrap()),
            "when every hop is a trusted proxy fall back to the left-most"
        );
    }
}
