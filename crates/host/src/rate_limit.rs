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
//!
//! The client address is **never** parsed from forwarded headers here. Trusted
//! proxy ranges are configured once in the `ClientInfo` section, resolved once by
//! the `ClientInfo` middleware (installed before this one), and this middleware
//! only reads [`IHttpContext::client_info`] — so the configuration-section path
//! and the `RateLimitPolicy` path always limit the same client.

use crate::problem_response::write_problem;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::net::IpAddr;
use std::ops::ControlFlow;
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
    /// Retained only by the legacy [`RateLimiter::with_trust_proxy`]
    /// compatibility shell; the limiter keys by the address it is handed and
    /// never reads this flag.
    #[allow(dead_code)]
    trust_proxy: bool,
}

impl RateLimiter {
    pub fn new(requests_per_second: f64, burst_size: u32, max_tracked_ips: usize) -> Self {
        Self {
            inner: InMemoryRateLimiter::new(requests_per_second, burst_size, max_tracked_ips),
            trust_proxy: false,
        }
    }

    /// Legacy compatibility shell (since 2.8.0 it is a no-op).
    ///
    /// Forwarded-header trust is configured once in the `ClientInfo` section
    /// (`TrustForwardedHeaders` / `TrustedProxies`) and resolved by the
    /// `ClientInfo` middleware; [`RateLimitMiddleware`] always reads
    /// [`IHttpContext::client_info`] and this flag is ignored. Kept so existing
    /// call sites keep compiling; planned removal in 3.0.
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

/// Rate-limiting middleware over a [`RateLimitPolicy`].
///
/// The IP subject is read from [`IHttpContext::client_info`]: without a
/// `ClientInfo` middleware that is the peer address, with one it is the resolved
/// trusted client address. Both construction paths ([`Self::from_policy`] and
/// [`Self::from_config`]) therefore behave identically; the legacy
/// `RateLimit.TrustProxy` / `RateLimit.TrustedProxies` keys are not consulted.
pub struct RateLimitMiddleware {
    policy: RateLimitPolicy,
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
        Self { policy }
    }

    /// Whether the policy has no rules (the middleware then always passes).
    pub fn is_empty(&self) -> bool {
        self.policy.is_empty()
    }

    /// Build the default IP rule from the `RateLimit` configuration section.
    ///
    /// The legacy `TrustProxy` / `TrustedProxies` keys are ignored: the IP
    /// subject reads the resolved [`IHttpContext::client_info`], whose trusted
    /// proxy ranges come from the `ClientInfo` section.
    pub fn from_config(cfg: &RateLimitSection) -> Self {
        let limiter: Arc<dyn IRateLimiter> = Arc::new(InMemoryRateLimiter::new(
            cfg.requests_per_second,
            cfg.burst_size,
            cfg.max_tracked_ips,
        ));
        Self::from_policy(RateLimitPolicy::new().with_rule(RateLimitRule::new(
            RateLimitRoute::Any,
            RateLimitSubject::Ip,
            limiter,
        )))
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
        let Some(key) = resolve_subject_key(ctx, rule.subject) else {
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
///
/// The client address is the resolved [`IHttpContext::client_info`] value — the
/// peer address by default, or the trusted forwarded client when the `ClientInfo`
/// middleware ran. This function never parses forwarded headers itself.
fn resolve_subject_key(ctx: &dyn IHttpContext, subject: RateLimitSubject) -> Option<String> {
    let ip_key = || ctx.client_info().client_ip.map(|ip| format!("ip:{ip}"));
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

    #[tokio::test]
    async fn trust_proxy_compat_shell_is_accepted_and_ignored() {
        // The pre-2.8 compatibility shell still compiles and limits; the flag it
        // stores is never read (the middleware keys by `ctx.client_info()`).
        let trusted = RateLimiter::new(100.0, 10, 10).with_trust_proxy(true);
        let plain = RateLimiter::new(100.0, 10, 10);
        let ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));
        assert!(trusted.allow(ip).await);
        assert!(plain.allow(ip).await);
        assert_eq!(trusted.retry_after_secs(), plain.retry_after_secs());
    }
}
