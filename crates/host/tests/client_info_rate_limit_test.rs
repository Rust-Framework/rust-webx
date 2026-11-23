//! WEBX-02: forwarded-header parsing has a single source of truth.
//!
//! The `ClientInfo` middleware resolves the trusted client once; the rate limiter
//! only reads [`IHttpContext::client_info`] and never parses headers a second
//! time. These tests pin the acceptance scenarios:
//!
//! a) with `ClientInfo.TrustForwardedHeaders` and trusted proxies configured, two
//!    forwarded clients behind the same proxy get independent buckets;
//! b) a directly reachable client cannot spoof `X-Forwarded-For` into another
//!    bucket — the peer address stays the key;
//! c) the `RateLimitPolicy` path and the `RateLimit` configuration-section path
//!    key the same resolved client and produce identical decisions; the legacy
//!    `RateLimit.TrustProxy` keys only produce a migration warning;
//! d) without the `ClientInfo` middleware the limiter falls back to the peer.

mod test_utils;

use std::net::SocketAddr;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};

use test_utils::TestHttpContext;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use webx_core::config::{ClientInfoSection, RateLimitSection};
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;
use webx_core::rate_limit::{
    IRateLimiter, RateLimitDecision, RateLimitPolicy, RateLimitRoute, RateLimitRule,
    RateLimitSubject,
};
use webx_host::client_info::ClientInfoMiddleware;
use webx_host::rate_limit::{InMemoryRateLimiter, RateLimitMiddleware};

const PATH: &str = "/api/orders";

fn peer(ip: &str) -> SocketAddr {
    SocketAddr::new(ip.parse().expect("valid peer ip"), 40_000)
}

fn request(peer_ip: &str, forwarded_for: Option<&str>) -> TestHttpContext {
    let ctx = TestHttpContext::new("GET", PATH).with_remote_addr(peer(peer_ip));
    match forwarded_for {
        Some(value) => ctx.with_header("x-forwarded-for", value),
        None => ctx,
    }
}

fn trusted_proxy_config() -> ClientInfoSection {
    ClientInfoSection {
        trust_forwarded_headers: true,
        trusted_proxies: vec!["10.0.0.0/8".into()],
    }
}

fn ip_policy(requests_per_second: f64, burst_size: u32) -> RateLimitPolicy {
    RateLimitPolicy::new().with_rule(RateLimitRule::new(
        RateLimitRoute::Any,
        RateLimitSubject::Ip,
        Arc::new(InMemoryRateLimiter::new(
            requests_per_second,
            burst_size,
            1_000,
        )),
    ))
}

fn ip_middleware(requests_per_second: f64, burst_size: u32) -> RateLimitMiddleware {
    RateLimitMiddleware::from_policy(ip_policy(requests_per_second, burst_size))
}

/// Run the real `ClientInfo` middleware, as the host pipeline does.
async fn resolve_client_info(ctx: &mut TestHttpContext, config: &ClientInfoSection) {
    let middleware = ClientInfoMiddleware::from_section(config);
    let flow = middleware
        .invoke(ctx)
        .await
        .expect("client info middleware cannot fail");
    assert!(matches!(flow, ControlFlow::Continue(())));
}

/// `true` = allowed, `false` = 429.
async fn check(middleware: &RateLimitMiddleware, ctx: &mut TestHttpContext) -> bool {
    let flow = middleware
        .invoke(ctx)
        .await
        .expect("rate limit middleware cannot fail");
    match flow {
        ControlFlow::Continue(()) => true,
        ControlFlow::Break(()) => false,
    }
}

#[tokio::test]
async fn trusted_proxy_for_forwarded_clients_gets_independent_buckets() {
    let config = trusted_proxy_config();
    // No refill and burst 1: every key gets exactly one request.
    let limiter = ip_middleware(0.0, 1);

    let mut client_a = request("10.0.0.5", Some("203.0.113.7"));
    resolve_client_info(&mut client_a, &config).await;
    assert_eq!(
        client_a.client_info().client_ip,
        Some("203.0.113.7".parse().unwrap()),
        "the forwarded client is the resolved client_ip"
    );
    assert!(check(&limiter, &mut client_a).await);

    // A different forwarded client behind the same proxy has its own bucket.
    let mut client_b = request("10.0.0.5", Some("203.0.113.8"));
    resolve_client_info(&mut client_b, &config).await;
    assert!(check(&limiter, &mut client_b).await);

    // The same forwarded client maps to the same bucket, now exhausted.
    let mut client_a_again = request("10.0.0.5", Some("203.0.113.7"));
    resolve_client_info(&mut client_a_again, &config).await;
    assert!(!check(&limiter, &mut client_a_again).await);
    assert_eq!(
        client_a_again.into_response_parts().0,
        429,
        "the throttled response contract stays 429"
    );
}

#[tokio::test]
async fn direct_client_cannot_spoof_forwarded_for_into_its_bucket() {
    let config = trusted_proxy_config();
    let limiter = ip_middleware(0.0, 1);

    let mut first = request("203.0.113.9", Some("198.51.100.1"));
    resolve_client_info(&mut first, &config).await;
    let info = first.client_info();
    assert_eq!(
        info.client_ip,
        Some("203.0.113.9".parse().unwrap()),
        "an untrusted peer stays the client"
    );
    assert!(
        info.forwarded_for.is_empty(),
        "client-controlled headers never surface as trusted"
    );
    assert!(check(&limiter, &mut first).await);

    // Same direct peer, different spoofed header -> still the same bucket.
    let mut second = request("203.0.113.9", Some("198.51.100.2"));
    resolve_client_info(&mut second, &config).await;
    assert!(
        !check(&limiter, &mut second).await,
        "the peer address is the bucket key, not the spoofed header"
    );
}

/// Delegates to an in-memory limiter while recording the resolved bucket keys.
struct RecordingLimiter {
    inner: InMemoryRateLimiter,
    keys: Arc<Mutex<Vec<String>>>,
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

#[tokio::test]
async fn from_policy_and_from_config_use_the_same_resolved_client_key() {
    // The legacy trust keys still bind for configuration compatibility, but
    // neither construction path consults them: trust lives in `ClientInfo`.
    let config: RateLimitSection = serde_json::from_value(serde_json::json!({
        "Enabled": true,
        "RequestsPerSecond": 0.0,
        "BurstSize": 1,
        "MaxTrackedIps": 1000,
        "TrustProxy": true,
        "TrustedProxies": ["10.0.0.0/8"],
    }))
    .expect("legacy trust keys still deserialize");
    let from_config = RateLimitMiddleware::from_config(&config);

    let recorded = Arc::new(Mutex::new(Vec::new()));
    let from_policy =
        RateLimitMiddleware::from_policy(RateLimitPolicy::new().with_rule(RateLimitRule::new(
            RateLimitRoute::Any,
            RateLimitSubject::Ip,
            Arc::new(RecordingLimiter {
                inner: InMemoryRateLimiter::new(0.0, 1, 1_000),
                keys: Arc::clone(&recorded),
            }),
        )));

    // Same resolved client sequence through both middlewares: first request for
    // forwarded client A, one for B, then A again.
    let client_info = trusted_proxy_config();
    let sequence = [
        ("10.0.0.5", "203.0.113.7"),
        ("10.0.0.5", "203.0.113.8"),
        ("10.0.0.5", "203.0.113.7"),
    ];
    let mut config_decisions = Vec::new();
    for (peer_ip, forwarded) in sequence {
        let mut ctx = request(peer_ip, Some(forwarded));
        resolve_client_info(&mut ctx, &client_info).await;
        config_decisions.push(check(&from_config, &mut ctx).await);
    }
    let mut policy_decisions = Vec::new();
    for (peer_ip, forwarded) in sequence {
        let mut ctx = request(peer_ip, Some(forwarded));
        resolve_client_info(&mut ctx, &client_info).await;
        policy_decisions.push(check(&from_policy, &mut ctx).await);
    }

    // Identical decisions imply identical bucket keys (same store contract).
    assert_eq!(config_decisions, policy_decisions);
    assert_eq!(config_decisions, vec![true, true, false]);
    // The policy path exposes the key it used: the resolved forwarded client.
    assert_eq!(
        *recorded.lock().expect("recorded keys poisoned"),
        vec!["ip:203.0.113.7", "ip:203.0.113.8", "ip:203.0.113.7"]
    );

    // The legacy keys are ignored by `from_config` itself too: without the
    // `ClientInfo` middleware the peer address is the only key, so a spoofed
    // header does not create a second bucket.
    let mut direct = request("203.0.113.30", Some("198.51.100.1"));
    assert!(check(&from_config, &mut direct).await);
    let mut direct_spoofed = request("203.0.113.30", Some("198.51.100.2"));
    assert!(!check(&from_config, &mut direct_spoofed).await);
}

#[tokio::test]
async fn without_client_info_middleware_the_limiter_uses_the_peer_address() {
    let limiter = ip_middleware(0.0, 1);

    // `IHttpContext::client_info` defaults to the peer address; headers are not
    // even looked at.
    let mut first = request("203.0.113.20", Some("1.1.1.1"));
    assert_eq!(
        first.client_info().client_ip,
        Some("203.0.113.20".parse().unwrap())
    );
    assert!(check(&limiter, &mut first).await);

    let mut second = request("203.0.113.20", Some("2.2.2.2"));
    assert!(!check(&limiter, &mut second).await);

    // No peer address at all: no usable subject, so the request passes instead
    // of every anonymous client sharing one global bucket.
    let mut anonymous = TestHttpContext::new("GET", PATH).with_header("x-forwarded-for", "1.1.1.1");
    assert_eq!(anonymous.client_info().client_ip, None);
    assert!(check(&limiter, &mut anonymous).await);
}

/// Captures WARN messages emitted while a host is assembled.
#[derive(Clone, Default)]
struct WarnCapture(Arc<Mutex<Vec<String>>>);

impl WarnCapture {
    fn messages(&self) -> Vec<String> {
        self.0.lock().expect("warn capture poisoned").clone()
    }
}

impl<S: Subscriber> Layer<S> for WarnCapture {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        if *event.metadata().level() != Level::WARN {
            return;
        }
        struct MessageVisitor<'a>(&'a mut String);
        impl Visit for MessageVisitor<'_> {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    *self.0 = format!("{value:?}");
                }
            }
            fn record_str(&mut self, field: &Field, value: &str) {
                if field.name() == "message" {
                    *self.0 = value.to_string();
                }
            }
        }
        let mut message = String::new();
        event.record(&mut MessageVisitor(&mut message));
        self.0.lock().expect("warn capture poisoned").push(message);
    }
}

#[test]
fn legacy_rate_limit_trust_keys_only_warn_during_assembly() {
    let capture = WarnCapture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    let host = tracing::subscriber::with_default(subscriber, || {
        webx_host::server::Host::builder()
            .mode(webx_core::mode::AppMode::Development)
            .no_spa()
            .configure(|app| {
                app.use_options(|options| {
                    // Legacy keys are accepted for configuration compatibility
                    // but are not read: they only trigger the migration warning.
                    options.rate_limit.trust_proxy = true;
                    options.rate_limit.trusted_proxies = vec!["10.0.0.0/8".into()];
                    // The warning must not turn rate limiting on by itself.
                    options.rate_limit.enabled = false;
                });
            })
            .build()
    });
    drop(host);

    let warnings = capture.messages().join("\n");
    assert!(
        warnings.contains("RateLimit.TrustProxy") && warnings.contains("ClientInfo"),
        "expected the legacy-trust migration warning pointing at ClientInfo, got: {warnings}"
    );
}
