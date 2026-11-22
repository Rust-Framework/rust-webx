//! Token-bucket rate-limiting middleware with a pluggable counter store.
//!
//! The default store is an in-process, **sharded** token bucket keyed by client
//! IP: sharding removes the single global mutex from the hot path while
//! `RateLimit.MaxTrackedIps` remains an exact global bound.
//!
//! A distributed deployment implements [`IRateLimiter`] over a shared store
//! (for example Redis) and injects it with [`RateLimitMiddleware::with_limiter`];
//! the middleware contract (`429` + `Retry-After` + `problem+json`) is unchanged.
//!
//! Configure via `appsettings.json` `RateLimit` section or manually:
//!
//! ```ignore
//! .use_middleware_with(|| {
//!     Arc::new(RateLimitMiddleware::from_config(&options.rate_limit))
//! })
//! ```

use crate::problem_response::write_problem;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::net::IpAddr;
use std::ops::ControlFlow;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use webx_core::config::RateLimitSection;
use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;
use webx_core::rate_limit::{
    IRateLimiter, RateLimitDecision, RateLimitPolicy, RateLimitRoute, RateLimitRule,
    RateLimitSubject,
};

struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    /// Insertion generation, used to discard stale FIFO entries when an IP is
    /// evicted and later reappears.
    generation: u64,
}

impl TokenBucket {
    fn new(burst_size: f64, generation: u64) -> Self {
        Self {
            tokens: burst_size,
            last_refill: Instant::now(),
            generation,
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

/// One shard of the in-process store: its own map, FIFO and exact capacity.
struct Shard {
    map: HashMap<String, TokenBucket>,
    /// FIFO of (key, generation). Eviction pops the front instead of scanning
    /// every tracked key (which made a key-cycling client O(max_keys) per
    /// request).
    order: VecDeque<(String, u64)>,
    next_generation: u64,
    capacity: usize,
}

impl Shard {
    fn consume(&mut self, key: &str, rate: f64, burst: f64) -> bool {
        if !self.map.contains_key(key) {
            while self.map.len() >= self.capacity {
                let Some((candidate, generation)) = self.order.pop_front() else {
                    break;
                };
                // Only remove the exact bucket that entered the FIFO here; the
                // key may have been evicted and re-inserted since.
                if self
                    .map
                    .get(&candidate)
                    .is_some_and(|bucket| bucket.generation == generation)
                {
                    self.map.remove(&candidate);
                    break;
                }
            }
            let generation = self.next_generation;
            self.next_generation = self.next_generation.wrapping_add(1);
            self.order.push_back((key.to_string(), generation));
            self.map
                .insert(key.to_string(), TokenBucket::new(burst, generation));
        }
        self.map
            .get_mut(key)
            .expect("bucket was just inserted")
            .try_consume(rate, burst)
    }
}

/// In-process, sharded token buckets implementing [`IRateLimiter`].
///
/// Shard count follows the available parallelism (clamped to `1..=32`) and is
/// never larger than `MaxTrackedIps`; capacities are distributed across shards
/// without remainder loss, so the tracked-key count never exceeds
/// `max_tracked_ips`.
///
/// Eviction is **per shard**: under adversarial key skew one shard can evict
/// before the global budget is reached, so a healthy key may lose its bucket.
/// That is the price of removing the global lock; deployments that need strict
/// per-key retention must provide an [`IRateLimiter`] backed by a shared store
/// with its own eviction policy.
pub struct InMemoryRateLimiter {
    shards: Vec<Mutex<Shard>>,
    rate: f64,
    burst: f64,
    retry_after_secs: u64,
}

impl InMemoryRateLimiter {
    pub fn new(requests_per_second: f64, burst_size: u32, max_tracked_ips: usize) -> Self {
        let max = max_tracked_ips.max(1);
        let desired = std::thread::available_parallelism()
            .map(|parallelism| parallelism.get() * 2)
            .unwrap_or(8)
            .clamp(1, 32);
        let shard_count = desired.min(max).max(1);
        let base = max / shard_count;
        let remainder = max % shard_count;
        let shards = (0..shard_count)
            .map(|index| {
                let capacity = base + usize::from(index < remainder);
                Mutex::new(Shard {
                    map: HashMap::new(),
                    order: VecDeque::new(),
                    next_generation: 0,
                    capacity: capacity.max(1),
                })
            })
            .collect();
        Self {
            shards,
            rate: requests_per_second,
            burst: burst_size as f64,
            retry_after_secs: retry_after_seconds(requests_per_second),
        }
    }

    fn shard_index(&self, key: &str) -> usize {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        (hasher.finish() as usize) % self.shards.len()
    }

    /// Tracked keys across all shards (diagnostics/tests).
    pub fn tracked_keys(&self) -> usize {
        let mut total = 0;
        for shard in &self.shards {
            total += shard
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .map
                .len();
        }
        total
    }

    /// Seconds a client should wait before retrying (RFC 6585 `Retry-After`).
    pub fn retry_after_secs(&self) -> u64 {
        self.retry_after_secs
    }
}

#[async_trait::async_trait]
impl IRateLimiter for InMemoryRateLimiter {
    async fn check(&self, key: &str) -> RateLimitDecision {
        let index = self.shard_index(key);
        let mut shard = self.shards[index]
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if shard.consume(key, self.rate, self.burst) {
            RateLimitDecision::allow()
        } else {
            RateLimitDecision::deny(self.retry_after_secs)
        }
    }
}

/// Whole-second refill hint; never `0` (that would invite a retry storm).
fn retry_after_seconds(rate: f64) -> u64 {
    if rate <= 0.0 {
        return 1;
    }
    ((1.0 / rate).ceil() as u64).max(1)
}

/// Compatibility wrapper for the pre-2.2 single-bucket API.
///
/// New code should depend on [`IRateLimiter`] / [`InMemoryRateLimiter`].
pub struct RateLimiter {
    inner: InMemoryRateLimiter,
    trust_proxy: bool,
}

impl RateLimiter {
    pub fn new(requests_per_second: f64, burst_size: u32, max_tracked_ips: usize) -> Self {
        Self {
            inner: InMemoryRateLimiter::new(requests_per_second, burst_size, max_tracked_ips),
            trust_proxy: false,
        }
    }

    pub fn with_trust_proxy(mut self, trust: bool) -> Self {
        self.trust_proxy = trust;
        self
    }

    pub async fn allow(&self, ip: IpAddr) -> bool {
        self.inner.check(&ip.to_string()).await.allowed
    }

    /// Current number of tracked client keys (for tests).
    pub async fn tracked_ip_count(&self) -> usize {
        self.inner.tracked_keys()
    }

    pub fn retry_after_secs(&self) -> u64 {
        self.inner.retry_after_secs()
    }
}

pub struct RateLimitMiddleware {
    policy: RateLimitPolicy,
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
        Self::with_limiter(Arc::new(InMemoryRateLimiter::new(
            requests_per_second,
            burst_size,
            max_tracked_ips,
        )))
    }

    /// Use a custom store (for example a Redis-backed distributed limiter).
    ///
    /// The store becomes one global rule keyed by the client IP, matching the
    /// pre-authentication middleware position where no claims are available.
    pub fn with_limiter(limiter: Arc<dyn IRateLimiter>) -> Self {
        Self::from_policy(RateLimitPolicy::new().with_rule(RateLimitRule::new(
            RateLimitRoute::Any,
            RateLimitSubject::Ip,
            limiter,
        )))
    }

    /// Rate-limit with per-route/subject rules.
    ///
    /// The host installs this after authentication (so `User`/`Tenant`/`ApiKey`
    /// subjects resolve); rule routes match the raw request path.
    pub fn from_policy(policy: RateLimitPolicy) -> Self {
        Self {
            policy,
            trust_proxy: false,
            trusted_proxies: Vec::new(),
        }
    }

    /// Whether the policy has no rules (the middleware then always passes).
    pub fn is_empty(&self) -> bool {
        self.policy.is_empty()
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
        let limiter: Arc<dyn IRateLimiter> = Arc::new(InMemoryRateLimiter::new(
            cfg.requests_per_second,
            cfg.burst_size,
            cfg.max_tracked_ips,
        ));
        Self {
            policy: RateLimitPolicy::new().with_rule(RateLimitRule::new(
                RateLimitRoute::Any,
                RateLimitSubject::Ip,
                limiter,
            )),
            trust_proxy: cfg.trust_proxy,
            trusted_proxies,
        }
    }
}

/// A parsed CIDR range used to identify trusted reverse proxies.
#[derive(Debug, Clone, Copy)]
pub(crate) struct IpCidr {
    network: IpAddr,
    prefix: u8,
}

impl IpCidr {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let (addr, prefix) = value.split_once('/')?;
        let network = IpAddr::from_str(addr.trim()).ok()?;
        let prefix: u8 = prefix.trim().parse().ok()?;
        let max_prefix = match network {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        (prefix <= max_prefix).then_some(Self { network, prefix })
    }

    pub(crate) fn contains(&self, ip: IpAddr) -> bool {
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
        let Some(rule) = self.policy.resolve(ctx.request().path()) else {
            return Ok(ControlFlow::Continue(()));
        };
        // No usable subject means the host cannot tell clients apart. Falling
        // back to a constant would turn per-client limiting into one global
        // bucket, so the limiter passes the request through.
        let Some(key) =
            resolve_subject_key(ctx, rule.subject, self.trust_proxy, &self.trusted_proxies)
        else {
            tracing::debug!("rate limit skipped: request has no usable subject");
            return Ok(ControlFlow::Continue(()));
        };

        let decision = rule.limiter.check(&key).await;
        if !decision.allowed {
            ctx.response_mut()
                .set_header("retry-after", &decision.retry_after_secs.to_string());
            write_problem(ctx, 429, "Too Many Requests").await;
            return Ok(ControlFlow::Break(()));
        }

        Ok(ControlFlow::Continue(()))
    }
}

/// Resolve the budget key for `subject`, falling back to the client IP when the
/// principal/credential is absent (fail-closed: anonymous traffic still gets
/// limited, just per address instead of per user).
fn resolve_subject_key(
    ctx: &dyn IHttpContext,
    subject: RateLimitSubject,
    trust_proxy: bool,
    trusted_proxies: &[IpCidr],
) -> Option<String> {
    let ip_key =
        || extract_client_ip(ctx, trust_proxy, trusted_proxies).map(|ip| format!("ip:{ip}"));
    match subject {
        RateLimitSubject::Ip => ip_key(),
        RateLimitSubject::User => claims_subject(ctx)
            .map(|subject| format!("user:{subject}"))
            .or_else(ip_key),
        RateLimitSubject::Tenant => claims_tenant(ctx)
            .map(|tenant| format!("tenant:{tenant}"))
            .or_else(ip_key),
        RateLimitSubject::ApiKey => presented_credential(ctx)
            .map(|credential| format!("key:{}", hash_credential(credential)))
            .or_else(ip_key),
    }
}

fn claims_subject(ctx: &dyn IHttpContext) -> Option<String> {
    ctx.claims()
        .map(|claims| claims.subject().to_string())
        .filter(|subject| !subject.is_empty())
}

fn claims_tenant(ctx: &dyn IHttpContext) -> Option<String> {
    ctx.claims()
        .and_then(|claims| claims.get_tenantid())
        .map(str::to_string)
        .filter(|tenant| !tenant.is_empty())
}

/// The presented credential: `x-api-key`, or a `Bearer` authorization value.
fn presented_credential(ctx: &dyn IHttpContext) -> Option<&str> {
    if let Some(value) = ctx.request().header("x-api-key") {
        if !value.trim().is_empty() {
            return Some(value.trim());
        }
    }
    let authorization = ctx.request().header("authorization")?;
    let (scheme, token) = authorization.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
        .filter(|token| !token.is_empty())
}

/// Hash the credential so the raw API key never becomes a store key.
fn hash_credential(credential: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(credential.as_bytes()))
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
pub(crate) fn client_from_forwarded_for(value: &str, trusted_proxies: &[IpCidr]) -> Option<IpAddr> {
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

    #[tokio::test]
    async fn burst_is_enforced_per_key() {
        let limiter = InMemoryRateLimiter::new(0.0, 2, 16);
        assert!(limiter.check("ip-a").await.allowed);
        assert!(limiter.check("ip-a").await.allowed);
        let denied = limiter.check("ip-a").await;
        assert!(!denied.allowed);
        assert_eq!(denied.retry_after_secs, 1);
        // Another key has its own bucket.
        assert!(limiter.check("ip-b").await.allowed);
    }

    #[tokio::test]
    async fn sharded_store_never_exceeds_the_global_key_budget() {
        let limiter = InMemoryRateLimiter::new(100.0, 10, 2);
        for index in 0..16 {
            assert!(limiter.check(&format!("10.0.0.{index}")).await.allowed);
            assert!(
                limiter.tracked_keys() <= 2,
                "the sharded store must stay within the configured budget"
            );
        }
        assert!(limiter.tracked_keys() >= 1);
    }

    #[tokio::test]
    async fn single_shard_eviction_is_exact() {
        // Tiny budgets collapse to one shard, so the bound is exact and the
        // oldest key is evicted first.
        let limiter = InMemoryRateLimiter::new(100.0, 10, 1);
        assert!(limiter.check("a").await.allowed);
        assert!(limiter.check("b").await.allowed);
        assert_eq!(limiter.tracked_keys(), 1);
    }

    #[tokio::test]
    async fn a_custom_port_replaces_the_default_store() {
        struct DenyAll;

        #[async_trait::async_trait]
        impl IRateLimiter for DenyAll {
            async fn check(&self, _key: &str) -> RateLimitDecision {
                RateLimitDecision::deny(7)
            }
        }

        let limiter: Arc<dyn IRateLimiter> = Arc::new(DenyAll);
        let decision = limiter.check("ip:203.0.113.7").await;
        assert!(!decision.allowed);
        assert_eq!(decision.retry_after_secs, 7);

        // The middleware accepts any store through the port.
        let _middleware = RateLimitMiddleware::with_limiter(limiter);
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
