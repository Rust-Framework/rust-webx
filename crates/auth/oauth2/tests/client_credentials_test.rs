//! Offline tests for the OAuth 2.0 `client_credentials` grant.
//!
//! The fake implements [`IHttpClient`] — the only transport seam — and answers
//! with a buffered [`OutboundResponse`] (`ok_response` /
//! `OutboundResponse::from_bytes`). It records each request through the
//! `OutboundRequest` read accessors so the tests assert the wire shape
//! (method, absolute URL, form body, headers, per-request deadline). Nothing
//! here opens a socket.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::{watch, Barrier};
use webx_client::testing::ok_response;
use webx_client::{HttpClientError, IHttpClient, OutboundRequest, OutboundResponse};
use webx_core::security::IClientCredentials;
use webx_core::time::{IClock, TestClock};
use webx_oauth2::{
    ClientCredentialsClient, ClientCredentialsConfig, ClientSecretAuth, ServiceToken,
};

const TOKEN_URL: &str = "https://id.example.com/token";
/// Frozen clock start (2023-11-14T22:13:20Z).
const NOW: u64 = 1_700_000_000;

// ---------------------------------------------------------------- test double

/// One scripted token-endpoint outcome.
enum Script {
    /// `200` with this JSON body.
    Body(&'static str),
    /// Arbitrary status with this body.
    Status(u16, &'static str),
    /// A transport failure (the endpoint was never reached).
    Transport(HttpClientError),
}

/// One recorded token request, as the endpoint would receive it.
#[derive(Clone)]
struct Recorded {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
    timeout: Option<Duration>,
}

struct FakeHttp {
    scripts: Mutex<VecDeque<Script>>,
    requests: Mutex<Vec<Recorded>>,
    count: AtomicUsize,
    /// When set, every request blocks until the channel value becomes `true`;
    /// used to force real overlap in the single-flight test.
    release: Mutex<Option<watch::Receiver<bool>>>,
}

impl FakeHttp {
    fn new(scripts: Vec<Script>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            requests: Mutex::new(Vec::new()),
            count: AtomicUsize::new(0),
            release: Mutex::new(None),
        }
    }

    fn with_gate(scripts: Vec<Script>, release: watch::Receiver<bool>) -> Self {
        let fake = Self::new(scripts);
        *fake.release.lock().expect("release lock") = Some(release);
        fake
    }

    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("requests lock").clone()
    }

    fn only_request(&self) -> Recorded {
        let mut requests = self.requests();
        assert_eq!(requests.len(), 1, "expected exactly one token request");
        requests.pop().expect("one request")
    }
}

#[async_trait]
impl IHttpClient for FakeHttp {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn execute(
        &self,
        request: OutboundRequest,
    ) -> std::result::Result<OutboundResponse, HttpClientError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        let body = request
            .body_bytes()
            .expect("a token request carries a buffered form body");
        self.requests.lock().expect("requests lock").push(Recorded {
            method: request.method().to_string(),
            path: request.path().to_string(),
            headers: request.headers().to_vec(),
            body: String::from_utf8_lossy(body).into_owned(),
            timeout: request.request_timeout(),
        });
        // Clone the receiver out: no `std` guard is held across this await.
        let release = self.release.lock().expect("release lock").clone();
        if let Some(mut release) = release {
            if !*release.borrow_and_update() {
                let _ = release.changed().await;
            }
        }
        let script = self
            .scripts
            .lock()
            .expect("scripts lock")
            .pop_front()
            .expect("a scripted token response");
        match script {
            Script::Body(body) => Ok(ok_response(body)),
            Script::Status(status, body) => OutboundResponse::from_bytes(
                status,
                vec![("content-type".to_string(), "application/json".to_string())],
                body.as_bytes(),
            ),
            Script::Transport(error) => Err(error),
        }
    }
}

/// A request that outlives the deadline it was handed: it sleeps past
/// `request_timeout()` and then reports the timeout the real `HttpClient` would
/// have produced.
struct TimeoutHttp {
    deadline: Mutex<Option<Duration>>,
}

impl TimeoutHttp {
    fn new() -> Self {
        Self {
            deadline: Mutex::new(None),
        }
    }

    fn deadline(&self) -> Option<Duration> {
        *self.deadline.lock().expect("deadline lock")
    }
}

#[async_trait]
impl IHttpClient for TimeoutHttp {
    fn name(&self) -> &str {
        "timeout"
    }

    async fn execute(
        &self,
        request: OutboundRequest,
    ) -> std::result::Result<OutboundResponse, HttpClientError> {
        let timeout = request
            .request_timeout()
            .expect("the client must set a per-request deadline");
        *self.deadline.lock().expect("deadline lock") = Some(timeout);
        tokio::time::sleep(timeout + Duration::from_millis(1)).await;
        Err(HttpClientError::Timeout(timeout))
    }
}

/// An HTTP client that ignores the per-request deadline hint: it stalls far
/// past it and, if the client ever let it finish, would answer successfully.
/// This models a transport that does not implement `request_timeout()`, so only
/// the client's own deadline can bound the call.
struct StalledHttp {
    stall: Duration,
    count: AtomicUsize,
    deadline: Mutex<Option<Duration>>,
}

impl StalledHttp {
    fn new(stall: Duration) -> Self {
        Self {
            stall,
            count: AtomicUsize::new(0),
            deadline: Mutex::new(None),
        }
    }

    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    fn deadline(&self) -> Option<Duration> {
        *self.deadline.lock().expect("deadline lock")
    }
}

#[async_trait]
impl IHttpClient for StalledHttp {
    fn name(&self) -> &str {
        "stalled"
    }

    async fn execute(
        &self,
        request: OutboundRequest,
    ) -> std::result::Result<OutboundResponse, HttpClientError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        *self.deadline.lock().expect("deadline lock") = request.request_timeout();
        // No timeout of its own: the hint was recorded and then ignored.
        tokio::time::sleep(self.stall).await;
        Ok(ok_response(
            r#"{"access_token":"tok-late","expires_in":3600}"#,
        ))
    }
}

/// An HTTP client that is only used to build/validate a client: no request is
/// ever sent through it.
fn unused_http() -> Arc<dyn IHttpClient> {
    Arc::new(FakeHttp::new(Vec::new()))
}

// -------------------------------------------------------------------- helpers

fn config(scopes: &[&str]) -> ClientCredentialsConfig {
    ClientCredentialsConfig::new(
        TOKEN_URL,
        "svc-client",
        "client-secret",
        scopes.iter().copied(),
    )
}

fn worker(
    config: ClientCredentialsConfig,
    fake: Arc<FakeHttp>,
    clock: Arc<dyn IClock>,
) -> ClientCredentialsClient {
    ClientCredentialsClient::new(config, fake, clock).expect("client builds")
}

/// `Arc<TestClock>` as the injected `Arc<dyn IClock>` (the tests keep the
/// concrete handle so they can advance time).
fn as_clock(clock: &Arc<TestClock>) -> Arc<dyn IClock> {
    let concrete: Arc<TestClock> = Arc::clone(clock);
    concrete
}

fn header(recorded: &Recorded, name: &str) -> Option<String> {
    recorded
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

fn form_pairs(recorded: &Recorded) -> std::collections::HashMap<String, String> {
    url::form_urlencoded::parse(recorded.body.as_bytes())
        .into_owned()
        .collect()
}

fn base64_encode(value: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(value)
}

// ---------------------------------------------------------------------- tests

#[tokio::test]
async fn success_posts_the_grant_with_basic_auth_and_the_declared_parameters() {
    let fake = Arc::new(FakeHttp::new(vec![Script::Body(
        r#"{"access_token":"tok-1","token_type":"Bearer","expires_in":3600}"#,
    )]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = worker(
        ClientCredentialsConfig::new(TOKEN_URL, "svc client", "s3cret:value", ["write", "read"])
            .with_audience("https://api.example.com")
            .with_client_auth(ClientSecretAuth::Basic),
        Arc::clone(&fake),
        clock,
    );

    let token = client.token().await.expect("token");
    assert_eq!(token.access_token, "tok-1");
    assert_eq!(token.expires_at_unix_secs, NOW + 3600);
    assert_eq!(
        token.scope, "read write",
        "the requested scopes are echoed when the response omits `scope`"
    );

    let recorded = fake.only_request();
    assert_eq!(recorded.method, "POST", "the token request is a POST");
    assert_eq!(recorded.path, TOKEN_URL, "the endpoint is used verbatim");
    assert_eq!(
        recorded.body,
        "grant_type=client_credentials&scope=read+write&audience=https%3A%2F%2Fapi.example.com"
    );
    // RFC 6749 §2.3.1: both halves are form-urlencoded before base64.
    let expected = format!("Basic {}", base64_encode("svc+client:s3cret%3Avalue"));
    assert_eq!(
        header(&recorded, "authorization").as_deref(),
        Some(expected.as_str())
    );
    assert_eq!(
        header(&recorded, "content-type").as_deref(),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        header(&recorded, "accept").as_deref(),
        Some("application/json")
    );
    let pairs = form_pairs(&recorded);
    assert_eq!(
        pairs.get("grant_type").map(String::as_str),
        Some("client_credentials")
    );
    assert_eq!(
        pairs.get("audience").map(String::as_str),
        Some("https://api.example.com")
    );
    assert!(
        !pairs.contains_key("client_id") && !pairs.contains_key("client_secret"),
        "Basic auth must not duplicate the credentials in the body: {pairs:?}"
    );
}

#[tokio::test]
async fn a_cached_token_is_served_without_a_second_request() {
    let fake = Arc::new(FakeHttp::new(vec![Script::Body(
        r#"{"access_token":"tok-1","token_type":"Bearer","expires_in":3600}"#,
    )]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = worker(config(&["read"]), Arc::clone(&fake), as_clock(&clock));

    let first = client.token().await.expect("first token");
    let second = client.token().await.expect("second token");
    assert_eq!(first.access_token, second.access_token);
    assert_eq!(first.expires_at_unix_secs, second.expires_at_unix_secs);
    assert_eq!(fake.count(), 1, "the cache hit must not reach the endpoint");
}

#[tokio::test]
async fn the_token_is_refreshed_once_the_refresh_ahead_window_is_entered() {
    let fake = Arc::new(FakeHttp::new(vec![
        Script::Body(r#"{"access_token":"tok-1","expires_in":300}"#),
        Script::Body(r#"{"access_token":"tok-2","expires_in":300}"#),
    ]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = worker(config(&["read"]), Arc::clone(&fake), as_clock(&clock));

    assert_eq!(client.token().await.expect("first").access_token, "tok-1");
    // expires_at (NOW + 300) - refresh_ahead (60s) = NOW + 240.
    clock.advance(Duration::from_secs(239));
    assert_eq!(client.token().await.expect("cached").access_token, "tok-1");
    assert_eq!(fake.count(), 1);

    clock.advance(Duration::from_secs(1));
    assert_eq!(
        client.token().await.expect("refreshed").access_token,
        "tok-2"
    );
    assert_eq!(fake.count(), 2);
}

#[tokio::test]
async fn refresh_ahead_is_clamped_to_half_of_a_short_lifetime() {
    let fake = Arc::new(FakeHttp::new(vec![
        Script::Body(r#"{"access_token":"tok-1","expires_in":20}"#),
        Script::Body(r#"{"access_token":"tok-2","expires_in":20}"#),
    ]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = worker(config(&["read"]), Arc::clone(&fake), as_clock(&clock));

    assert_eq!(client.token().await.expect("first").access_token, "tok-1");
    // 60s of skew on a 20s token would refresh instantly; the clamp is 10s.
    clock.advance(Duration::from_secs(9));
    assert_eq!(client.token().await.expect("cached").access_token, "tok-1");
    clock.advance(Duration::from_secs(1));
    assert_eq!(
        client.token().await.expect("refreshed").access_token,
        "tok-2"
    );
    assert_eq!(fake.count(), 2);
}

#[tokio::test]
async fn concurrent_callers_share_one_in_flight_request() {
    let (release, released) = watch::channel(false);
    let fake = Arc::new(FakeHttp::with_gate(
        vec![Script::Body(
            r#"{"access_token":"tok-1","token_type":"Bearer","expires_in":3600}"#,
        )],
        released,
    ));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = Arc::new(worker(
        config(&["read"]),
        Arc::clone(&fake),
        as_clock(&clock),
    ));

    let barrier = Arc::new(Barrier::new(9));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let client = Arc::clone(&client);
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            client.token().await
        }));
    }
    barrier.wait().await;

    // Wait until the leader is inside the endpoint call, then give the other
    // callers a chance to reach the per-key lock before releasing it.
    tokio::time::timeout(Duration::from_secs(5), async {
        while fake.count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("a token request must start");
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    release.send(true).expect("release the leader");

    for handle in handles {
        assert_eq!(
            handle.await.expect("join").expect("token").access_token,
            "tok-1"
        );
    }
    assert_eq!(fake.count(), 1, "one in-flight request per cache key");
}

#[tokio::test]
async fn a_rejected_client_is_a_configuration_defect_and_is_not_cached() {
    let fake = Arc::new(FakeHttp::new(vec![
        Script::Status(401, r#"{"error":"invalid_client"}"#),
        Script::Body(r#"{"access_token":"tok-2","expires_in":600}"#),
    ]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = worker(config(&["read"]), Arc::clone(&fake), as_clock(&clock));

    let error = client
        .token()
        .await
        .expect_err("rejected credentials must fail");
    assert_eq!(
        error.status_code(),
        500,
        "a credential/configuration defect is a server-side error"
    );
    assert_eq!(
        error.code(),
        "dependency_injection",
        "Error::di is ErrorKind::Di in the rust-core baseline"
    );
    assert!(!error.retryable(), "a 4xx verdict must not be retried");
    assert!(
        error
            .operator()
            .unwrap_or_default()
            .contains("invalid_client"),
        "the provider error identifier is kept for operators: {error:?}"
    );
    assert_eq!(fake.count(), 1);

    let token = client
        .token()
        .await
        .expect("a later attempt is not served from the failure");
    assert_eq!(token.access_token, "tok-2");
    assert_eq!(fake.count(), 2, "failures are never cached");
}

#[tokio::test]
async fn endpoint_outages_are_retryable_and_not_cached() {
    let fake = Arc::new(FakeHttp::new(vec![
        Script::Status(503, r#"{"error":"temporarily_unavailable"}"#),
        Script::Body(r#"{"access_token":"tok-2","expires_in":600}"#),
    ]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = worker(config(&["read"]), Arc::clone(&fake), as_clock(&clock));

    let error = client.token().await.expect_err("an outage must fail");
    assert_eq!(error.status_code(), 503);
    assert_eq!(error.code(), "service_unavailable");
    assert!(error.retryable());

    assert_eq!(client.token().await.expect("retry").access_token, "tok-2");
    assert_eq!(fake.count(), 2, "an outage must not be cached");
}

#[tokio::test]
async fn a_rate_limited_endpoint_is_retryable_and_not_cached() {
    let fake = Arc::new(FakeHttp::new(vec![
        Script::Status(429, r#"{"error":"temporarily_unavailable"}"#),
        Script::Body(r#"{"access_token":"tok-2","expires_in":600}"#),
    ]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = worker(config(&["read"]), Arc::clone(&fake), as_clock(&clock));

    let error = client.token().await.expect_err("throttling must fail");
    assert_eq!(
        error.status_code(),
        503,
        "429 is the endpoint throttling us, not a verdict on the credentials"
    );
    assert_eq!(error.code(), "service_unavailable");
    assert!(error.retryable());

    assert_eq!(client.token().await.expect("retry").access_token, "tok-2");
    assert_eq!(fake.count(), 2, "a throttled attempt must not be cached");
}

#[tokio::test]
async fn the_di_port_shares_one_singleton_cache() {
    let fake = Arc::new(FakeHttp::new(vec![
        Script::Body(r#"{"access_token":"tok-1","expires_in":3600}"#),
        Script::Body(r#"{"access_token":"tok-2","expires_in":3600}"#),
    ]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client: Arc<ClientCredentialsClient> = Arc::new(worker(
        config(&["read"]),
        Arc::clone(&fake),
        as_clock(&clock),
    ));

    // What the composition root registers: one `Arc<dyn IClientCredentials>`.
    let registered: Arc<ClientCredentialsClient> = Arc::clone(&client);
    let port: Arc<dyn IClientCredentials> = registered;
    let from_port = port.token(&["read"]).await.expect("port token");
    let from_client = client.token().await.expect("inherent token");
    assert_eq!(from_port.access_token, from_client.access_token);
    assert_eq!(
        fake.count(),
        1,
        "the port and the client must share the same keyed cache"
    );

    // An empty slice is "no scope parameter", a separate bucket.
    let unscoped = port.token(&[]).await.expect("unscoped token");
    assert_eq!(unscoped.access_token, "tok-2");
    assert_eq!(unscoped.scope, "");
    assert_eq!(fake.count(), 2);
    assert!(
        !form_pairs(&fake.requests()[1]).contains_key("scope"),
        "the port must forward the empty slice as \"no scope parameter\""
    );
}

#[tokio::test]
async fn transport_failures_are_retryable() {
    for script in [
        Script::Transport(HttpClientError::Timeout(Duration::from_secs(10))),
        Script::Transport(HttpClientError::Connect("dns".to_string())),
        Script::Transport(HttpClientError::BodyTooLarge { limit: 65536 }),
    ] {
        let fake = Arc::new(FakeHttp::new(vec![script]));
        let clock = Arc::new(TestClock::from_unix(NOW));
        let client = worker(config(&["read"]), Arc::clone(&fake), as_clock(&clock));

        let error = client
            .token()
            .await
            .expect_err("a transport failure must fail");
        assert_eq!(error.status_code(), 503);
        assert_eq!(error.code(), "service_unavailable");
        assert!(error.retryable());
        assert!(
            !error.public_detail().unwrap_or_default().contains("65536"),
            "transport text must not leak into the client-visible detail"
        );
    }
}

#[tokio::test]
async fn a_request_that_outlives_its_deadline_is_retryable() {
    let http = Arc::new(TimeoutHttp::new());
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = ClientCredentialsClient::new(
        // Clamped up to the 1s floor: the fake sleeps just past that budget.
        config(&["read"]).with_timeout(Duration::from_millis(100)),
        http.clone(),
        as_clock(&clock),
    )
    .expect("client builds");

    let error = client
        .token()
        .await
        .expect_err("a request past its deadline must fail");
    assert_eq!(error.status_code(), 503);
    assert_eq!(error.code(), "service_unavailable");
    assert!(error.retryable());
    // `Error::unavailable` keeps its detail client-visible, but it is the
    // framework's own text, never the transport message.
    assert!(
        error
            .public_detail()
            .unwrap_or_default()
            .contains("timed out"),
        "the deadline is reported without leaking transport text: {error:?}"
    );
    assert_eq!(
        http.deadline(),
        Some(Duration::from_secs(1)),
        "the fake slept past the clamped 1s deadline the client handed it"
    );
}

#[tokio::test]
async fn a_stalled_request_is_bounded_by_the_deadline() {
    // The configured 200 ms is clamped up to the 1s floor; the transport stalls
    // for 2s and never reports a timeout of its own.
    let stall = Duration::from_secs(2);
    let http = Arc::new(StalledHttp::new(stall));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = ClientCredentialsClient::new(
        config(&["read"]).with_timeout(Duration::from_millis(200)),
        http.clone(),
        as_clock(&clock),
    )
    .expect("client builds");

    let started = Instant::now();
    let error = client
        .token()
        .await
        .expect_err("a stalled exchange must fail at the deadline");
    let elapsed = started.elapsed();

    assert_eq!(
        error.status_code(),
        503,
        "a deadline is transient: {error:?}"
    );
    assert_eq!(error.code(), "service_unavailable");
    assert!(error.retryable());
    assert!(
        error
            .public_detail()
            .unwrap_or_default()
            .contains("timed out"),
        "the deadline is reported without leaking transport text: {error:?}"
    );
    assert_eq!(
        http.count(),
        1,
        "the timed-out exchange is reported, not retried internally"
    );
    assert_eq!(
        http.deadline(),
        Some(Duration::from_secs(1)),
        "the transport hint is still handed to the client, clamped to the floor"
    );
    assert!(
        elapsed < stall,
        "the client deadline, not the stalled transport, bounded the call: {elapsed:?}"
    );
}

#[tokio::test]
async fn malformed_or_unusable_token_responses_are_rejected() {
    for body in [
        r#"{"access_token":"","token_type":"Bearer"}"#,
        r#"{"access_token":"   ","token_type":"Bearer"}"#,
        r#"{"token_type":"Bearer","expires_in":3600}"#,
        r#"{"access_token":"tok","token_type":"MAC"}"#,
        r#"{"access_token":"tok","token_type":7}"#,
        "not json",
        "",
    ] {
        let fake = Arc::new(FakeHttp::new(vec![Script::Body(body)]));
        let clock = Arc::new(TestClock::from_unix(NOW));
        let client = worker(config(&["read"]), Arc::clone(&fake), as_clock(&clock));

        let error = client
            .token()
            .await
            .expect_err("the response must be rejected");
        assert_eq!(error.status_code(), 503, "response body: {body}");
        assert!(error.retryable(), "response body: {body}");
    }
}

#[tokio::test]
async fn plaintext_endpoints_are_rejected_unless_private_networks_are_allowed() {
    let clock = Arc::new(TestClock::from_unix(NOW));
    let insecure = ClientCredentialsConfig::new(
        "http://127.0.0.1:9000/token",
        "svc-client",
        "client-secret",
        ["read"],
    );

    let error = ClientCredentialsClient::new(insecure.clone(), unused_http(), as_clock(&clock))
        .expect_err("plaintext must fail closed");
    assert_eq!(error.status_code(), 500);
    assert_eq!(error.code(), "dependency_injection");

    ClientCredentialsClient::new(
        insecure.allow_private_networks(true),
        unused_http(),
        as_clock(&clock),
    )
    .expect("loopback is allowed with the explicit opt-in");

    ClientCredentialsClient::new(config(&["read"]), unused_http(), as_clock(&clock))
        .expect("https is always accepted");

    for endpoint in [
        "id.example.com/token",
        "ftp://id.example.com/token",
        "http://id.example.com/token",
        "",
    ] {
        let invalid =
            ClientCredentialsConfig::new(endpoint, "svc-client", "client-secret", ["read"]);
        assert!(
            ClientCredentialsClient::new(invalid, unused_http(), as_clock(&clock)).is_err(),
            "endpoint '{endpoint}' must be rejected"
        );
    }
}

#[tokio::test]
async fn invalidate_drops_the_cached_token() {
    let fake = Arc::new(FakeHttp::new(vec![
        Script::Body(r#"{"access_token":"tok-1","expires_in":3600}"#),
        Script::Body(r#"{"access_token":"tok-2","expires_in":3600}"#),
    ]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = worker(config(&["read"]), Arc::clone(&fake), as_clock(&clock));

    assert_eq!(client.token().await.expect("first").access_token, "tok-1");
    assert_eq!(client.token().await.expect("cached").access_token, "tok-1");
    assert_eq!(fake.count(), 1);

    client.invalidate().await;
    assert_eq!(
        client.token().await.expect("after invalidate").access_token,
        "tok-2"
    );
    assert_eq!(fake.count(), 2);
}

#[tokio::test]
async fn token_cache_is_bounded() {
    const CAP: usize = 256;
    const SCOPE_COUNT: usize = 300;
    // Caller-chosen scope sets are distinct cache keys; the fake must answer
    // every request that is actually sent (including the post-eviction refetch).
    let scripts = (0..=SCOPE_COUNT)
        .map(|_| Script::Body(r#"{"access_token":"tok","expires_in":3600}"#))
        .collect();
    let fake = Arc::new(FakeHttp::new(scripts));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = worker(config(&["read"]), Arc::clone(&fake), as_clock(&clock));

    for index in 0..SCOPE_COUNT {
        let scope = format!("scope-{index}");
        client
            .token_for(&[scope.as_str()])
            .await
            .expect("a token per scope set");
    }
    assert_eq!(
        fake.count(),
        SCOPE_COUNT,
        "every new scope set is requested"
    );
    assert_eq!(
        client.cached_entries().await,
        CAP,
        "the cache must not grow past its bound"
    );

    // The earliest key was evicted, so it is fetched again ...
    client.token_for(&["scope-0"]).await.expect("refetch");
    assert_eq!(fake.count(), SCOPE_COUNT + 1);
    assert_eq!(client.cached_entries().await, CAP);

    // ... while the most recently inserted key is still served from the cache.
    client.token_for(&["scope-299"]).await.expect("cached");
    assert_eq!(
        fake.count(),
        SCOPE_COUNT + 1,
        "the newest bucket must still be a cache hit"
    );

    client.invalidate().await;
    assert_eq!(
        client.cached_entries().await,
        0,
        "invalidate drops the entries and the eviction order together"
    );
}

#[tokio::test]
async fn token_for_overrides_the_scope_set_and_an_empty_slice_omits_the_parameter() {
    let fake = Arc::new(FakeHttp::new(vec![
        Script::Body(r#"{"access_token":"tok-1","expires_in":3600}"#),
        Script::Body(r#"{"access_token":"tok-2","expires_in":3600}"#),
        Script::Body(r#"{"access_token":"tok-3","expires_in":3600}"#),
    ]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = worker(config(&["read"]), Arc::clone(&fake), as_clock(&clock));

    assert_eq!(
        client.token().await.expect("configured").access_token,
        "tok-1"
    );

    let admin = client
        .token_for(&["admin", "read"])
        .await
        .expect("explicit scopes");
    assert_eq!(admin.scope, "admin read");

    let unscoped = client.token_for(&[]).await.expect("no scopes");
    assert_eq!(unscoped.scope, "", "no scope was requested");

    let requests = fake.requests();
    assert_eq!(
        form_pairs(&requests[0]).get("scope").map(String::as_str),
        Some("read")
    );
    assert_eq!(
        form_pairs(&requests[1]).get("scope").map(String::as_str),
        Some("admin read")
    );
    assert!(
        !form_pairs(&requests[2]).contains_key("scope"),
        "an empty slice must not send a scope parameter"
    );
    assert_eq!(fake.count(), 3, "each scope set is its own cache bucket");

    // The configured bucket is still valid: no extra request.
    assert_eq!(client.token().await.expect("cached").access_token, "tok-1");
    assert_eq!(fake.count(), 3);
}

#[tokio::test]
async fn client_secret_post_puts_the_credentials_in_the_form() {
    let fake = Arc::new(FakeHttp::new(vec![Script::Body(
        r#"{"access_token":"tok-1","expires_in":3600}"#,
    )]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = worker(
        ClientCredentialsConfig::new(TOKEN_URL, "svc client", "s3cret:value", ["read"])
            .with_client_auth(ClientSecretAuth::Post),
        Arc::clone(&fake),
        clock,
    );

    client.token().await.expect("token");

    let recorded = fake.only_request();
    assert_eq!(recorded.method, "POST");
    assert_eq!(recorded.path, TOKEN_URL);
    assert!(header(&recorded, "authorization").is_none());
    assert_eq!(
        header(&recorded, "content-type").as_deref(),
        Some("application/x-www-form-urlencoded")
    );
    assert!(recorded.body.contains("client_secret=s3cret%3Avalue"));
    let pairs = form_pairs(&recorded);
    assert_eq!(
        pairs.get("grant_type").map(String::as_str),
        Some("client_credentials")
    );
    assert_eq!(
        pairs.get("client_id").map(String::as_str),
        Some("svc client")
    );
    assert_eq!(
        pairs.get("client_secret").map(String::as_str),
        Some("s3cret:value")
    );
}

#[tokio::test]
async fn a_missing_expires_in_falls_back_to_three_hundred_seconds() {
    let fake = Arc::new(FakeHttp::new(vec![Script::Body(
        r#"{"access_token":"tok-1"}"#,
    )]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = worker(config(&["read"]), fake, clock);

    let token = client.token().await.expect("token");
    assert_eq!(token.expires_at_unix_secs, NOW + 300);
}

#[tokio::test]
async fn a_scope_echo_from_the_endpoint_wins_over_the_request() {
    let fake = Arc::new(FakeHttp::new(vec![Script::Body(
        r#"{"access_token":"tok-1","expires_in":100,"scope":"read write extra"}"#,
    )]));
    let clock = Arc::new(TestClock::from_unix(NOW));
    let client = worker(config(&["read"]), fake, clock);

    assert_eq!(
        client.token().await.expect("token").scope,
        "read write extra"
    );
}

#[tokio::test]
async fn the_request_deadline_is_clamped_to_one_through_sixty_seconds() {
    for (requested, expected) in [
        (Duration::from_millis(100), Duration::from_secs(1)),
        (Duration::from_secs(10), Duration::from_secs(10)),
        (Duration::from_secs(300), Duration::from_secs(60)),
    ] {
        let fake = Arc::new(FakeHttp::new(vec![Script::Body(
            r#"{"access_token":"tok-1","token_type":"Bearer","expires_in":3600}"#,
        )]));
        let clock = Arc::new(TestClock::from_unix(NOW));
        let client = worker(
            config(&["read"]).with_timeout(requested),
            Arc::clone(&fake),
            clock,
        );
        client.token().await.expect("token");
        assert_eq!(
            fake.only_request().timeout,
            Some(expected),
            "requested {requested:?}"
        );
    }
}

#[test]
fn debug_output_redacts_the_secret_and_the_access_token() {
    let config = ClientCredentialsConfig::new(TOKEN_URL, "svc-client", "super-secret", ["read"]);
    let debug = format!("{config:?}");
    assert!(debug.contains("<redacted>"));
    assert!(debug.contains(TOKEN_URL));
    assert!(!debug.contains("super-secret"), "leaked: {debug}");

    let token = ServiceToken {
        access_token: "tok-secret".to_string(),
        expires_at_unix_secs: NOW,
        scope: "read".to_string(),
    };
    let debug = format!("{token:?}");
    assert!(debug.contains("<redacted>"));
    assert!(debug.contains("\"read\""));
    assert!(!debug.contains("tok-secret"), "leaked: {debug}");
}
