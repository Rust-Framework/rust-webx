//! Rate-limit routing and subject composition:
//!
//! a) `RateLimitRoute::Pattern` templates match one route shape without widening
//!    to a prefix, and resolution keeps exact > pattern > longest prefix > `Any`;
//! b) `RateLimitSubject::Composite` budgets a combination such as `(tenant,
//!    user)`, with an explicit per-part IP fallback when a part is absent;
//! c) `token_bucket(...)` builds a rule from the host's `InMemoryRateLimiter`
//!    (no duplicated limiter implementation).

mod test_utils;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};

use test_utils::TestHttpContext;
use webx_core::auth::IClaims;
use webx_core::http::IClaimsExt;
use webx_core::middleware::IMiddleware;
use webx_core::rate_limit::{
    IRateLimiter, RateLimitDecision, RateLimitPolicy, RateLimitRoute, RateLimitRule,
    RateLimitSubject,
};
use webx_host::rate_limit::{
    token_bucket, InMemoryRateLimiter, RateLimitMiddleware, DEFAULT_MAX_TRACKED_KEYS,
};

fn context(path: &str, peer: &str) -> TestHttpContext {
    TestHttpContext::new("POST", path).with_remote_addr(SocketAddr::new(
        peer.parse().expect("valid peer ip"),
        40_000,
    ))
}

/// Minimal claims: a subject and the raw claims map the rate limiter reads
/// (`tenant_id`).
struct TestClaims {
    sub: String,
    values: HashMap<String, String>,
}

impl TestClaims {
    fn new(sub: &str, tenant: Option<&str>) -> Self {
        let mut values = HashMap::new();
        if let Some(tenant) = tenant {
            values.insert("tenant_id".to_string(), tenant.to_string());
        }
        Self {
            sub: sub.to_string(),
            values,
        }
    }
}

impl IClaims for TestClaims {
    fn subject(&self) -> &str {
        &self.sub
    }

    fn roles(&self) -> &[String] {
        &[]
    }

    fn permissions(&self) -> &[String] {
        &[]
    }

    fn claims(&self) -> &HashMap<String, String> {
        &self.values
    }

    fn clone_box(&self) -> Box<dyn IClaims> {
        Box::new(Self {
            sub: self.sub.clone(),
            values: self.values.clone(),
        })
    }
}

fn with_claims(mut ctx: TestHttpContext, sub: &str, tenant: Option<&str>) -> TestHttpContext {
    ctx.set_claims(Box::new(TestClaims::new(sub, tenant)));
    ctx
}

/// Delegates to the real in-process store while recording the resolved bucket
/// keys, so tests can assert which rule (and which subject) was applied.
struct RecordingLimiter {
    inner: InMemoryRateLimiter,
    keys: Arc<Mutex<Vec<String>>>,
}

impl RecordingLimiter {
    fn new(requests_per_second: f64, burst: u32) -> (Self, Arc<Mutex<Vec<String>>>) {
        let keys = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                inner: InMemoryRateLimiter::new(requests_per_second, burst, 1_000),
                keys: Arc::clone(&keys),
            },
            keys,
        )
    }
}

#[async_trait::async_trait]
impl IRateLimiter for RecordingLimiter {
    async fn check(&self, key: &str) -> RateLimitDecision {
        self.keys
            .lock()
            .expect("recorded keys poisoned")
            .push(key.to_string());
        self.inner.check(key).await
    }
}

fn recorded(keys: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    keys.lock().expect("recorded keys poisoned").clone()
}

/// `true` = allowed, `false` = 429.
async fn check(middleware: &RateLimitMiddleware, ctx: &mut TestHttpContext) -> bool {
    matches!(
        middleware
            .invoke(ctx)
            .await
            .expect("rate limit middleware cannot fail"),
        ControlFlow::Continue(())
    )
}

#[tokio::test]
async fn pattern_rule_only_covers_the_template_route() {
    let (limiter, keys) = RecordingLimiter::new(0.0, 1);
    let middleware =
        RateLimitMiddleware::from_policy(RateLimitPolicy::new().with_rule(RateLimitRule::new(
            RateLimitRoute::pattern("/api/tenants/{id}/invitations"),
            RateLimitSubject::Ip,
            Arc::new(limiter),
        )));

    // Two requests with different tenant ids share the pattern's IP bucket.
    let mut first = context("/api/tenants/42/invitations", "203.0.113.7");
    assert!(check(&middleware, &mut first).await);
    let mut second = context("/api/tenants/7/invitations", "203.0.113.7");
    assert!(!check(&middleware, &mut second).await);
    assert_eq!(second.into_response_parts().0, 429);

    // Neighbouring routes are not widened by the template.
    for path in [
        "/api/tenants/42",
        "/api/tenants/42/invitations/7",
        "/api/tenants//invitations",
        "/api/tenants/42/members",
    ] {
        let mut unmatched = context(path, "203.0.113.7");
        assert!(
            check(&middleware, &mut unmatched).await,
            "{path} must not be limited by the template rule"
        );
    }
    assert_eq!(
        recorded(&keys).len(),
        2,
        "only the two invitations requests reached the limiter"
    );
}

#[tokio::test]
async fn precedence_is_exact_then_pattern_then_longest_prefix() {
    let (exact, exact_keys) = RecordingLimiter::new(0.0, 10);
    let (pattern, pattern_keys) = RecordingLimiter::new(0.0, 10);
    let (prefix, prefix_keys) = RecordingLimiter::new(0.0, 10);
    let middleware = RateLimitMiddleware::from_policy(
        RateLimitPolicy::new()
            .with_rule(RateLimitRule::new(
                RateLimitRoute::prefix("/api/tenants"),
                RateLimitSubject::Ip,
                Arc::new(prefix),
            ))
            .with_rule(RateLimitRule::new(
                RateLimitRoute::pattern("/api/tenants/{id}/invitations"),
                RateLimitSubject::Ip,
                Arc::new(pattern),
            ))
            .with_rule(RateLimitRule::new(
                RateLimitRoute::exact("/api/tenants/42/invitations"),
                RateLimitSubject::Ip,
                Arc::new(exact),
            )),
    );

    // Matches all three rules: exact wins.
    let mut exact_request = context("/api/tenants/42/invitations", "203.0.113.7");
    assert!(check(&middleware, &mut exact_request).await);
    assert_eq!(recorded(&exact_keys).len(), 1);
    assert!(recorded(&pattern_keys).is_empty());
    assert!(recorded(&prefix_keys).is_empty());

    // Matches pattern and prefix: pattern wins.
    let mut pattern_request = context("/api/tenants/43/invitations", "203.0.113.7");
    assert!(check(&middleware, &mut pattern_request).await);
    assert_eq!(recorded(&pattern_keys).len(), 1);
    assert!(recorded(&prefix_keys).is_empty());

    // Matches only the prefix.
    let mut prefix_request = context("/api/tenants/43/members", "203.0.113.7");
    assert!(check(&middleware, &mut prefix_request).await);
    assert_eq!(recorded(&prefix_keys).len(), 1);
}

#[tokio::test]
async fn composite_subject_budgets_the_tenant_and_user_combination() {
    let (limiter, keys) = RecordingLimiter::new(0.0, 1);
    let middleware =
        RateLimitMiddleware::from_policy(RateLimitPolicy::new().with_rule(RateLimitRule::new(
            RateLimitRoute::Any,
            RateLimitSubject::Composite(&[RateLimitSubject::Tenant, RateLimitSubject::User]),
            Arc::new(limiter),
        )));

    // Same tenant, different users: one budget each.
    let mut alice = with_claims(
        context("/api/invitations", "203.0.113.7"),
        "alice",
        Some("tenant-1"),
    );
    assert!(check(&middleware, &mut alice).await);
    let mut bob = with_claims(
        context("/api/invitations", "203.0.113.7"),
        "bob",
        Some("tenant-1"),
    );
    assert!(check(&middleware, &mut bob).await);
    // Same user, other tenant: another budget.
    let mut alice_elsewhere = with_claims(
        context("/api/invitations", "203.0.113.8"),
        "alice",
        Some("tenant-2"),
    );
    assert!(check(&middleware, &mut alice_elsewhere).await);
    // Repeating one combination hits its own exhausted bucket.
    let mut alice_again = with_claims(
        context("/api/invitations", "203.0.113.7"),
        "alice",
        Some("tenant-1"),
    );
    assert!(!check(&middleware, &mut alice_again).await);

    assert_eq!(
        recorded(&keys),
        vec![
            // `(tenant-1, alice)`, then `(tenant-1, bob)`, then `(tenant-2, alice)`,
            // then `(tenant-1, alice)` again: part keys are length-prefixed.
            "composite:15:tenant:tenant-1|10:user:alice|",
            "composite:15:tenant:tenant-1|8:user:bob|",
            "composite:15:tenant:tenant-2|10:user:alice|",
            "composite:15:tenant:tenant-1|10:user:alice|",
        ]
    );
}

#[tokio::test]
async fn composite_missing_part_falls_back_to_the_client_ip() {
    let (limiter, keys) = RecordingLimiter::new(0.0, 1);
    let middleware =
        RateLimitMiddleware::from_policy(RateLimitPolicy::new().with_rule(RateLimitRule::new(
            RateLimitRoute::Any,
            RateLimitSubject::Composite(&[RateLimitSubject::Tenant, RateLimitSubject::User]),
            Arc::new(limiter),
        )));

    // Tenant present, no authenticated user: the user part is the client IP
    // (never skipped, which would widen the budget).
    let mut tenant_only = with_claims(
        context("/api/invitations", "203.0.113.9"),
        "",
        Some("tenant-1"),
    );
    assert!(check(&middleware, &mut tenant_only).await);
    assert_eq!(
        recorded(&keys),
        vec!["composite:15:tenant:tenant-1|14:ip:203.0.113.9|"]
    );

    // The same anonymous tenant request from the same address is limited.
    let mut tenant_only_again = with_claims(
        context("/api/invitations", "203.0.113.9"),
        "",
        Some("tenant-1"),
    );
    assert!(!check(&middleware, &mut tenant_only_again).await);

    // A different address has its own budget.
    let mut other_peer = with_claims(
        context("/api/invitations", "203.0.113.10"),
        "",
        Some("tenant-1"),
    );
    assert!(check(&middleware, &mut other_peer).await);

    // No claims and no peer address: no usable part at all, so the middleware
    // passes the request instead of creating one global bucket (same as a
    // single-subject rule without a subject).
    let mut unknown = TestHttpContext::new("POST", "/api/invitations");
    assert!(check(&middleware, &mut unknown).await);
}

#[tokio::test]
async fn nested_composites_flatten_to_the_same_budget_as_a_flat_slice() {
    const TENANT_USER: &[RateLimitSubject] = &[RateLimitSubject::Tenant, RateLimitSubject::User];
    static NESTED: &[RateLimitSubject] = &[
        RateLimitSubject::Composite(TENANT_USER),
        RateLimitSubject::Ip,
    ];
    static FLAT: &[RateLimitSubject] = &[
        RateLimitSubject::Tenant,
        RateLimitSubject::User,
        RateLimitSubject::Ip,
    ];

    let (nested_limiter, nested_keys) = RecordingLimiter::new(0.0, 1);
    let nested =
        RateLimitMiddleware::from_policy(RateLimitPolicy::new().with_rule(RateLimitRule::new(
            RateLimitRoute::Any,
            RateLimitSubject::Composite(NESTED),
            Arc::new(nested_limiter),
        )));
    let (flat_limiter, flat_keys) = RecordingLimiter::new(0.0, 1);
    let flat =
        RateLimitMiddleware::from_policy(RateLimitPolicy::new().with_rule(RateLimitRule::new(
            RateLimitRoute::Any,
            RateLimitSubject::Composite(FLAT),
            Arc::new(flat_limiter),
        )));

    let request = || {
        with_claims(
            context("/api/invitations", "203.0.113.7"),
            "alice",
            Some("tenant-1"),
        )
    };
    let mut nested_request = request();
    assert!(check(&nested, &mut nested_request).await);
    let mut flat_request = request();
    assert!(check(&flat, &mut flat_request).await);

    // A nested declaration budgets exactly like the equivalent flat slice.
    assert_eq!(recorded(&nested_keys), recorded(&flat_keys));
    assert_eq!(
        recorded(&nested_keys),
        vec!["composite:15:tenant:tenant-1|10:user:alice|14:ip:203.0.113.7|"]
    );

    // And the flattened rule is live: the same combination is throttled next.
    let mut nested_again = request();
    assert!(!check(&nested, &mut nested_again).await);
}

#[tokio::test]
async fn token_bucket_builds_a_dedicated_in_memory_budget() {
    let rule = token_bucket(
        RateLimitRoute::pattern("/api/tenants/{id}/invitations"),
        RateLimitSubject::Ip,
        0.0,
        2,
    );
    assert_eq!(
        rule.route,
        RateLimitRoute::pattern("/api/tenants/{id}/invitations")
    );
    assert_eq!(rule.subject, RateLimitSubject::Ip);
    assert_eq!(DEFAULT_MAX_TRACKED_KEYS, 10_000);

    assert!(rule.limiter.check("k").await.allowed);
    assert!(rule.limiter.check("k").await.allowed);
    let denied = rule.limiter.check("k").await;
    assert!(!denied.allowed);
    assert_eq!(denied.retry_after_secs, 1);

    // Every rule gets its own store.
    let other = token_bucket(RateLimitRoute::Any, RateLimitSubject::Ip, 0.0, 2);
    assert!(!Arc::ptr_eq(&rule.limiter, &other.limiter));
    assert!(other.limiter.check("k").await.allowed);
}

#[tokio::test]
async fn token_bucket_rule_applies_through_the_middleware() {
    let middleware =
        RateLimitMiddleware::from_policy(RateLimitPolicy::new().with_rule(token_bucket(
            RateLimitRoute::pattern("/api/tenants/{id}/invitations"),
            RateLimitSubject::Ip,
            0.0,
            1,
        )));

    let mut first = context("/api/tenants/1/invitations", "203.0.113.7");
    assert!(check(&middleware, &mut first).await);

    let mut second = context("/api/tenants/2/invitations", "203.0.113.7");
    assert!(!check(&middleware, &mut second).await);
    let (status, headers, body) = second.into_response_parts();
    assert_eq!(status, 429);
    assert!(
        headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("retry-after") && value == "1"),
        "the 429 must carry Retry-After"
    );
    let body = body.expect("problem+json body");
    assert!(
        String::from_utf8_lossy(&body).contains("Too Many Requests"),
        "the 429 keeps the problem+json contract"
    );
}
