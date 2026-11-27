//! Offline tests for the F4 external PDP authorization client.
//!
//! Every test but one answers through [`FakeHttp`], which implements
//! `IHttpClient` — the one transport seam — records the exact request through
//! `OutboundRequest`'s read accessors (`method`, `path`, `headers`,
//! `body_bytes`, `request_timeout`) and answers with a buffered
//! `OutboundResponse` built by `OutboundResponse::from_bytes` /
//! `webx_client::testing::ok_response`.
//!
//! `body_collection_is_inside_the_deadline` is the exception: a buffered
//! response hands its whole body over at once, so it cannot reproduce a PDP
//! that sends headers early and then stalls the body. That one test uses a real
//! loopback socket instead of growing a production-only response constructor.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use webx_client::testing::ok_response;
use webx_client::{
    HttpClient, HttpClientError, HttpClientOptions, IHttpClient, OutboundRequest, OutboundResponse,
};
use webx_core::authorization::{AccessCheck, AccessDecision, IAuthorizationClient};
use webx_core::error::Result;
use webx_core::mode::AppMode;
use webx_core::time::{IClock, TestClock};
use webx_host::{AuthorizationHttpConfig, Host, HttpAuthorizationClient};

/// A plaintext loopback endpoint. No socket is ever opened (the fake answers),
/// it only has to pass the `allow_private_networks` gate.
const ENDPOINT: &str = "http://127.0.0.1:9/v1/check";
/// Frozen clock start (2023-11-14T22:13:20Z).
const NOW: u64 = 1_700_000_000;
/// The client's documented decision-document cap (256 KiB).
const MAX_DECISION_BODY: usize = 256 * 1024;

// ---------------------------------------------------------------- test double

/// One recorded PDP request, as the endpoint would receive it.
#[derive(Debug, Clone)]
struct Recorded {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    timeout: Option<Duration>,
}

impl Recorded {
    /// Case-insensitive header lookup, like HTTP itself.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The buffered body. `None` would mean a streamed request, which the PDP
    /// client never sends.
    fn body(&self) -> &[u8] {
        self.body
            .as_deref()
            .expect("the PDP request body is buffered")
    }
}

/// Scripted [`IHttpClient`]: records requests, answers from a queue and counts
/// the calls it received.
struct FakeHttp {
    responses: Mutex<VecDeque<OutboundResponse>>,
    requests: Mutex<Vec<Recorded>>,
    calls: AtomicUsize,
    /// Latency applied to every answer: it stands in for a PDP that accepts the
    /// connection and then stops responding.
    latency: Duration,
}

impl FakeHttp {
    fn new(responses: Vec<OutboundResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
            latency: Duration::ZERO,
        }
    }

    /// The same fake, answering only after `latency`. The delay lives here (not
    /// in the request) precisely because the client must not trust the
    /// transport to honour `request_timeout`.
    fn stalling(responses: Vec<OutboundResponse>, latency: Duration) -> Self {
        Self {
            latency,
            ..Self::new(responses)
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests
            .lock()
            .expect("fake request log is not poisoned")
            .clone()
    }

    fn only_request(&self) -> Recorded {
        let requests = self.requests();
        assert_eq!(requests.len(), 1, "exactly one PDP request was expected");
        requests.into_iter().next().expect("one recorded request")
    }
}

#[async_trait]
impl IHttpClient for FakeHttp {
    fn name(&self) -> &str {
        "fake-pdp"
    }

    async fn execute(
        &self,
        request: OutboundRequest,
    ) -> std::result::Result<OutboundResponse, HttpClientError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let recorded = Recorded {
            method: request.method().to_string(),
            path: request.path().to_string(),
            headers: request.headers().to_vec(),
            body: request.body_bytes().map(<[u8]>::to_vec),
            timeout: request.request_timeout(),
        };
        self.requests
            .lock()
            .expect("fake request log is not poisoned")
            .push(recorded);

        let response = self
            .responses
            .lock()
            .expect("fake response queue is not poisoned")
            .pop_front()
            .expect("exactly the scripted number of PDP requests");
        if !self.latency.is_zero() {
            tokio::time::sleep(self.latency).await;
        }
        Ok(response)
    }
}

// -------------------------------------------------------------------- helpers

/// The test configuration: private networks allowed so the loopback endpoint
/// passes validation without opening a socket.
fn config() -> AuthorizationHttpConfig {
    AuthorizationHttpConfig::new(ENDPOINT).allow_private_networks(true)
}

/// A buffered `200 application/json` response carrying `body`.
fn decision_response(body: &'static [u8]) -> OutboundResponse {
    ok_response(body)
}

fn as_clock(clock: &Arc<TestClock>) -> Arc<dyn IClock> {
    let concrete: Arc<TestClock> = Arc::clone(clock);
    concrete
}

fn as_http(http: &Arc<FakeHttp>) -> Arc<dyn IHttpClient> {
    let concrete: Arc<FakeHttp> = Arc::clone(http);
    concrete
}

fn client(
    config: AuthorizationHttpConfig,
    http: &Arc<FakeHttp>,
    clock: &Arc<TestClock>,
) -> HttpAuthorizationClient {
    HttpAuthorizationClient::new(config, as_http(http), as_clock(clock)).expect("client builds")
}

fn check(subject: &str, permission: &str) -> AccessCheck {
    AccessCheck {
        subject: subject.to_string(),
        permission: permission.to_string(),
        ..AccessCheck::default()
    }
}

/// Failed PDP calls are retryable `503`s, never a decision.
fn assert_fail_closed<T: std::fmt::Debug>(result: Result<T>) {
    let error = result.expect_err("a PDP failure must never become an Allow");
    assert_eq!(error.status_code(), 503, "fail-closed is 503: {error}");
    assert_eq!(error.code(), "service_unavailable");
    assert!(error.retryable(), "a PDP outage must be retryable");
}

// ---------------------------------------------------------------------- tests

#[tokio::test]
async fn allow_and_deny_are_returned() {
    let fake = Arc::new(FakeHttp::new(vec![
        decision_response(br#"{"decision":"allow"}"#),
        // Case-insensitive per the wire contract.
        decision_response(br#"{"decision":"DENY"}"#),
    ]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = client(config(), &fake, &clock);

    assert_eq!(
        client
            .check(&check("alice", "orders.read"))
            .await
            .expect("the PDP allowed"),
        AccessDecision::Allow
    );
    assert_eq!(
        client
            .check(&check("bob", "orders.read"))
            .await
            .expect("the PDP denied"),
        AccessDecision::Deny
    );
    assert_eq!(fake.calls(), 2);
}

#[tokio::test]
async fn request_shape_is_stable() {
    let fake = Arc::new(FakeHttp::new(vec![
        decision_response(br#"{"decision":"allow"}"#),
        decision_response(br#"[{"decision":"allow"},{"decision":"deny"}]"#),
    ]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = client(config().with_bearer_token("pdp-token"), &fake, &clock);

    client
        .check(&check("alice", "orders.read"))
        .await
        .expect("allow");

    let request = fake.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, ENDPOINT);
    assert_eq!(
        request.timeout,
        Some(Duration::from_secs(2)),
        "the default 2s deadline must reach the HTTP client"
    );
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(
        request.header("accept"),
        Some("application/json"),
        "the PDP is asked for a JSON answer"
    );
    assert_eq!(
        request.header("authorization"),
        Some("Bearer pdp-token"),
        "the configured PDP credential is sent as a bearer token"
    );

    // Exact bytes: field names, order, `null` optionals and `{}` context.
    assert_eq!(
        std::str::from_utf8(request.body()).expect("UTF-8 body"),
        r#"{"subject":"alice","tenant_id":null,"permission":"orders.read","resource":null,"context":{}}"#
    );
    let body: serde_json::Value = serde_json::from_slice(request.body()).expect("the body is JSON");
    assert_eq!(body["subject"], "alice");
    assert_eq!(body["tenant_id"], serde_json::Value::Null);
    assert_eq!(body["resource"], serde_json::Value::Null);
    assert_eq!(body["context"], serde_json::json!({}));

    // A batch is an array of the same objects, in caller order.
    let decisions = client
        .check_batch(&[check("alice", "orders.read"), check("bob", "orders.write")])
        .await
        .expect("batch answer");
    assert_eq!(decisions, vec![AccessDecision::Allow, AccessDecision::Deny]);

    let requests = fake.requests();
    assert_eq!(requests.len(), 2);
    let batch: serde_json::Value =
        serde_json::from_slice(requests[1].body()).expect("batch body is JSON");
    let batch = batch.as_array().expect("batch body is an array");
    assert_eq!(batch.len(), 2);
    assert_eq!(batch[0]["subject"], "alice");
    assert_eq!(batch[1]["subject"], "bob");
    assert_eq!(batch[1]["tenant_id"], serde_json::Value::Null);
    assert_eq!(batch[1]["context"], serde_json::json!({}));
}

#[tokio::test]
async fn non_2xx_is_retryable_503() {
    // A 4xx from the PDP (rejected client credential) is an outage for this
    // caller, not a denial — and never an Allow, even if the body says so.
    for status in [401_u16, 403, 429, 500, 503] {
        let fake = Arc::new(FakeHttp::new(vec![OutboundResponse::from_bytes(
            status,
            vec![("content-type".into(), "application/json".into())],
            Bytes::from_static(br#"{"decision":"allow"}"#),
        )
        .expect("valid response")]));
        let clock = Arc::new(TestClock::from_unix(NOW));
        let client = client(config(), &fake, &clock);

        assert_fail_closed(client.check(&check("alice", "orders.read")).await);
        assert_eq!(fake.calls(), 1, "HTTP {status} must still be attempted");
    }
}

#[tokio::test]
async fn malformed_body_is_retryable_503() {
    let bodies: [&'static [u8]; 5] = [
        br#"{"allowed":true}"#,
        br#"{"decision":"maybe"}"#,
        br#"{"decision":true}"#,
        br#"<html>not json</html>"#,
        b"",
    ];
    for body in bodies {
        let fake = Arc::new(FakeHttp::new(vec![decision_response(body)]));
        let clock = Arc::new(TestClock::from_unix(NOW));
        let client = client(config(), &fake, &clock);

        assert_fail_closed(client.check(&check("alice", "orders.read")).await);
        assert_eq!(fake.calls(), 1, "a malformed answer is not retried inline");
    }

    // A batch answer whose length differs from the request is malformed too.
    let fake = Arc::new(FakeHttp::new(vec![decision_response(
        br#"[{"decision":"allow"}]"#,
    )]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = client(config(), &fake, &clock);
    assert_fail_closed(
        client
            .check_batch(&[check("alice", "a"), check("bob", "b")])
            .await,
    );
    assert_eq!(fake.calls(), 1);
}

#[tokio::test]
async fn oversized_body_is_retryable_503() {
    // A decision document is tiny; a huge answer is hostile or broken and must
    // not be buffered, let alone parsed.
    let huge = Bytes::from(vec![b'x'; MAX_DECISION_BODY + 1]);
    let fake = Arc::new(FakeHttp::new(vec![ok_response(huge)]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = client(config(), &fake, &clock);

    assert_fail_closed(client.check(&check("alice", "orders.read")).await);
    assert_eq!(fake.calls(), 1, "an oversized answer is not retried inline");
}

#[tokio::test]
async fn a_request_that_outlives_the_deadline_is_retryable_503() {
    // The fake accepts the request and never answers; the client's own deadline
    // must fire even though the fake ignores `request_timeout`.
    let fake = Arc::new(FakeHttp::stalling(
        vec![decision_response(br#"{"decision":"allow"}"#)],
        Duration::from_secs(5),
    ));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = client(
        config().with_timeout(Duration::from_millis(50)),
        &fake,
        &clock,
    );
    assert_eq!(
        client.config().timeout,
        Duration::from_millis(100),
        "the configured deadline is clamped up to the documented floor"
    );

    assert_fail_closed(client.check(&check("alice", "orders.read")).await);
    assert_eq!(
        fake.calls(),
        1,
        "a stalled check is attempted once and never retried inline"
    );
}

/// A loopback PDP that sends its response **headers** immediately and its
/// **body** only after `body_delay`, then returns its endpoint.
///
/// This is the M-6 shape: `IHttpClient::execute` returns as soon as the headers
/// arrive, so a deadline that only wraps `execute` leaves the body read (and
/// the parse after it) unbounded. A buffered `OutboundResponse` cannot express
/// this — its `ResponseStream` yields everything in one chunk — so the test
/// speaks HTTP/1.1 over a real loopback socket instead.
async fn slow_body_pdp(body_delay: Duration) -> String {
    const BODY: &[u8] = br#"{"decision":"allow"}"#;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback PDP");
    let address = listener.local_addr().expect("loopback address");
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                // The request head is all this fake needs; the body is
                // irrelevant.
                let mut head = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    let read = socket.read(&mut chunk).await.expect("read the request");
                    if read == 0 {
                        return;
                    }
                    head.extend_from_slice(&chunk[..read]);
                    if head.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                // Headers now: `execute` resolves here, with the body unread.
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                    BODY.len()
                );
                socket
                    .write_all(head.as_bytes())
                    .await
                    .expect("write the response head");
                socket.flush().await.expect("flush the response head");
                tokio::time::sleep(body_delay).await;
                // A client that honoured its deadline has already dropped the
                // stream; a failed write is the expected outcome.
                let _ = socket.write_all(BODY).await;
            });
        }
    });
    format!("http://{address}/v1/check")
}

#[tokio::test]
async fn body_collection_is_inside_the_deadline() {
    // The floor deadline is 100ms; the body arrives after a full second, so a
    // deadline that stops at the response headers would let this check take ten
    // times longer than configured (and then return `Allow`).
    let budget = Duration::from_millis(100);
    let body_delay = Duration::from_secs(1);
    let endpoint = slow_body_pdp(body_delay).await;
    let http: Arc<dyn IHttpClient> = Arc::new(
        HttpClient::new(
            "slow-body-pdp",
            HttpClientOptions {
                allow_private_networks: true,
                ..HttpClientOptions::new("http://127.0.0.1")
            },
        )
        .expect("the loopback HTTP client builds"),
    );
    let clock = Arc::new(TestClock::from_unix(NOW));
    let endpoint_config =
        || AuthorizationHttpConfig::new(endpoint.clone()).allow_private_networks(true);

    let bounded = HttpAuthorizationClient::new(
        endpoint_config().with_timeout(budget),
        Arc::clone(&http),
        as_clock(&clock),
    )
    .expect("client builds");
    let started = Instant::now();
    assert_fail_closed(bounded.check(&check("alice", "orders.read")).await);
    let elapsed = started.elapsed();
    assert!(
        elapsed < body_delay,
        "the deadline must cover the body read, not just the response headers; \
         the check waited {elapsed:?}"
    );

    // Control: the same endpoint really does answer `Allow` once the body is
    // allowed to arrive, so the `503` above was the deadline and not a broken
    // fake/transport.
    let patient = HttpAuthorizationClient::new(
        endpoint_config().with_timeout(Duration::from_secs(5)),
        http,
        as_clock(&clock),
    )
    .expect("client builds");
    assert_eq!(
        patient
            .check(&check("alice", "orders.read"))
            .await
            .expect("the body arrives before the 5s deadline"),
        AccessDecision::Allow
    );
}

#[tokio::test]
async fn cache_serves_allow_without_http() {
    let fake = Arc::new(FakeHttp::new(vec![
        decision_response(br#"{"decision":"allow"}"#),
        decision_response(br#"{"decision":"allow"}"#),
    ]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = client(
        config().with_cache_ttl(Duration::from_secs(30)),
        &fake,
        &clock,
    );
    let probe = check("alice", "orders.read");

    assert_eq!(
        client.check(&probe).await.expect("allow"),
        AccessDecision::Allow
    );
    assert_eq!(fake.calls(), 1);

    clock.advance(Duration::from_secs(29));
    assert_eq!(
        client.check(&probe).await.expect("cached allow"),
        AccessDecision::Allow
    );
    assert_eq!(fake.calls(), 1, "inside the TTL the cache answers");

    clock.advance(Duration::from_secs(2));
    assert_eq!(
        client.check(&probe).await.expect("refetched allow"),
        AccessDecision::Allow
    );
    assert_eq!(fake.calls(), 2, "an expired entry is refetched");
}

#[tokio::test]
async fn deny_is_not_cached() {
    let fake = Arc::new(FakeHttp::new(vec![
        decision_response(br#"{"decision":"deny"}"#),
        decision_response(br#"{"decision":"deny"}"#),
    ]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = client(
        config().with_cache_ttl(Duration::from_secs(300)),
        &fake,
        &clock,
    );
    let probe = check("alice", "orders.read");

    assert_eq!(
        client.check(&probe).await.expect("deny"),
        AccessDecision::Deny
    );
    assert_eq!(
        client.check(&probe).await.expect("deny again"),
        AccessDecision::Deny
    );
    assert_eq!(
        fake.calls(),
        2,
        "a revocation must take effect immediately: Deny is never cached"
    );
}

#[tokio::test]
async fn invalidate_forces_refetch() {
    let fake = Arc::new(FakeHttp::new(vec![
        decision_response(br#"{"decision":"allow"}"#),
        decision_response(br#"{"decision":"allow"}"#),
    ]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = client(
        config().with_cache_ttl(Duration::from_secs(300)),
        &fake,
        &clock,
    );
    let probe = check("alice", "orders.read");

    client.check(&probe).await.expect("allow");
    assert_eq!(fake.calls(), 1);

    client.invalidate();
    assert_eq!(
        client.check(&probe).await.expect("refetched allow"),
        AccessDecision::Allow
    );
    assert_eq!(fake.calls(), 2, "invalidate clears every cached decision");
}

#[tokio::test]
async fn batch_preserves_order_and_respects_max_batch() {
    let fake = Arc::new(FakeHttp::new(vec![decision_response(
        br#"[{"decision":"allow"},{"decision":"deny"}]"#,
    )]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = client(config().with_max_batch(2), &fake, &clock);

    let too_many = vec![check("a", "x"), check("b", "x"), check("c", "x")];
    let error = client
        .check_batch(&too_many)
        .await
        .expect_err("a batch over max_batch is a configuration error");
    assert_eq!(error.status_code(), 500);
    assert_eq!(error.code(), "dependency_injection");
    assert_eq!(fake.calls(), 0, "a rejected batch must not be sent");

    let decisions = client
        .check_batch(&[check("alice", "orders.read"), check("bob", "orders.write")])
        .await
        .expect("batch answer");
    assert_eq!(
        decisions,
        vec![AccessDecision::Allow, AccessDecision::Deny],
        "the answer order follows the request order"
    );
    assert_eq!(fake.calls(), 1);
    let batch: serde_json::Value =
        serde_json::from_slice(fake.only_request().body()).expect("batch body is JSON");
    assert_eq!(
        batch.as_array().expect("an array").len(),
        2,
        "one array element per check"
    );
}

#[tokio::test]
async fn di_registration_resolves_the_port() {
    let fake = Arc::new(FakeHttp::new(vec![decision_response(
        br#"{"decision":"allow"}"#,
    )]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let http: Arc<dyn IHttpClient> = fake.clone();
    let registered = Arc::new(
        HttpAuthorizationClient::new(config(), http, as_clock(&clock)).expect("client builds"),
    );

    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_authorization_client(registered)
        .try_build()
        .expect("host builds");

    let port = host
        .provider()
        .get::<dyn IAuthorizationClient>()
        .expect("use_authorization_client must register `Arc<dyn IAuthorizationClient>`");
    assert_eq!(
        port.check(&check("alice", "orders.read"))
            .await
            .expect("allow"),
        AccessDecision::Allow,
        "the resolved port is the registered client"
    );
    assert_eq!(fake.calls(), 1);
    assert_eq!(
        fake.only_request().header("content-type"),
        Some("application/json"),
        "the resolved port is the registered HTTP-backed client"
    );
}

#[tokio::test]
async fn non_https_endpoint_is_rejected() {
    let fake = Arc::new(FakeHttp::new(Vec::new()));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let http: Arc<dyn IHttpClient> = fake.clone();

    let error = HttpAuthorizationClient::new(
        AuthorizationHttpConfig::new("http://pdp.example.com/v1/check"),
        Arc::clone(&http),
        as_clock(&clock),
    )
    .expect_err("plaintext must be rejected unless private networks are allowed");
    assert_eq!(error.status_code(), 500);
    assert_eq!(error.code(), "dependency_injection");
    assert_eq!(fake.calls(), 0, "an invalid configuration sends nothing");

    let error = HttpAuthorizationClient::new(
        AuthorizationHttpConfig::new("ftp://pdp.example.com/v1/check"),
        Arc::clone(&http),
        as_clock(&clock),
    )
    .expect_err("unsupported schemes are configuration errors");
    assert_eq!(error.code(), "dependency_injection");

    let client = HttpAuthorizationClient::new(
        AuthorizationHttpConfig::new("http://127.0.0.1:9/v1/check").allow_private_networks(true),
        Arc::clone(&http),
        as_clock(&clock),
    )
    .expect("the explicit opt-in accepts a private plaintext endpoint");
    assert!(client.config().allow_private_networks);
}
