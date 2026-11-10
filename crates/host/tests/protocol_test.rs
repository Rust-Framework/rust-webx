//! Protocol tests for the single 401 write path (release/0.9.0).
//!
//! Contract under test: every unauthenticated request — whether rejected by
//! `#[authorize]`, by the identity binding layer, or by the JWT middleware — is
//! written by exactly one code path (`problem_response::write_unauthorized`), so
//! the response is always `401 application/problem+json` with a
//! `WWW-Authenticate: Bearer` challenge, and internal failure text never reaches
//! the client.
//!
//! `rust-webx-host` cannot depend on the `webx` umbrella crate (circular), so the
//! routes below are registered with hand-written `inventory::submit!` entries —
//! the same inventory items `#[get]` + `#[handler]` + `#[authorize]` generate —
//! and the dispatch functions mimic the code the macros emit.

#![allow(clippy::type_complexity)]

use std::net::TcpListener;
use std::time::Duration;

use webx_core::error::{Error, Result};
use webx_core::http::IHttpContext;
use webx_core::localization::TextRef;
use webx_core::route::scan::{
    AuthorizeRule, HandlerRegistration, ResponseData, RouteDispatch, RouteEntry,
};
use webx_core::routing::HttpMethod;

const TEST_SECRET: &[u8] = b"protocol-test-secret-key-for-single-401-path";

/// Declarative rules from `#[authorize]` — a bare attribute means "any
/// authenticated principal".
const AUTHENTICATED_ONLY: &[AuthorizeRule] = &[AuthorizeRule::authenticated()];

const AUTHZ_PATH: &str = "/test/protocol/authorized";
const BINDING_PATH: &str = "/test/protocol/binding";
const FAILING_PATH: &str = "/test/protocol/failing";
const PUBLIC_PATH: &str = "/test/protocol/public";

/// Return type every route dispatch function uses (`RouteDispatchFn` is the
/// function-pointer type that wraps it).
type DispatchFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<ResponseData>> + Send + 'a>>;

// ---------------------------------------------------------------------------
// Dispatch functions
// ---------------------------------------------------------------------------

/// Reached only when the declarative `#[authorize]` rules pass.
fn dispatch_authorized(_ctx: &mut dyn IHttpContext) -> DispatchFuture<'_> {
    Box::pin(async { ResponseData::json(&"authorized") })
}

/// Mirrors the binding code `#[derive(WebxRequestMeta)]` emits for a required
/// `#[fromclaim] user: IdentityUser` field: missing claims is an
/// `Error::Unauthorized` (401), not a 400 binding failure.
fn dispatch_binding(ctx: &mut dyn IHttpContext) -> DispatchFuture<'_> {
    let authenticated = ctx.claims().is_some();
    Box::pin(async move {
        if !authenticated {
            return Err(Error::Unauthorized(
                "authentication required for this endpoint".to_string(),
            ));
        }
        ResponseData::json(&"bound")
    })
}

/// Fails with an internal message that must never be echoed to the client.
fn dispatch_failing(_ctx: &mut dyn IHttpContext) -> DispatchFuture<'_> {
    Box::pin(async {
        let failed: Result<ResponseData> = Err(Error::Internal(
            "db connect failed for /var/lib/app/secret.db".to_string(),
        ));
        failed
    })
}

/// Public endpoint: no `#[authorize]`, no identity field. It never looks at the
/// claims, so a stale/invalid token must be irrelevant to it.
fn dispatch_public(_ctx: &mut dyn IHttpContext) -> DispatchFuture<'_> {
    Box::pin(async { ResponseData::json(&"public") })
}

// ---------------------------------------------------------------------------
// Minimal `#[handler]` bridge (diagnostics require every route to have one)
// ---------------------------------------------------------------------------

fn noop_factory(
    _resolver: &dyn rust_dix::IServiceResolver,
) -> Result<Box<dyn std::any::Any + Send>> {
    Ok(Box::new(()) as Box<dyn std::any::Any + Send>)
}

fn noop_call(
    _handler: Box<dyn std::any::Any + Send>,
    _request: Box<dyn std::any::Any + Send>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Box<dyn std::any::Any + Send>>> + Send>,
> {
    Box::pin(async { Ok(Box::new(()) as Box<dyn std::any::Any + Send>) })
}

inventory::submit! {
    HandlerRegistration {
        req_type_id: std::any::TypeId::of::<u8>(),
        req_type_name: "ProtocolAuthorizedRequest",
        factory: noop_factory,
        call: noop_call,
    }
}

inventory::submit! {
    HandlerRegistration {
        req_type_id: std::any::TypeId::of::<u16>(),
        req_type_name: "ProtocolBindingRequest",
        factory: noop_factory,
        call: noop_call,
    }
}

inventory::submit! {
    HandlerRegistration {
        req_type_id: std::any::TypeId::of::<u32>(),
        req_type_name: "ProtocolFailingRequest",
        factory: noop_factory,
        call: noop_call,
    }
}

inventory::submit! {
    HandlerRegistration {
        req_type_id: std::any::TypeId::of::<u64>(),
        req_type_name: "ProtocolPublicRequest",
        factory: noop_factory,
        call: noop_call,
    }
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

inventory::submit! {
    RouteDispatch {
        handler_type: "ProtocolAuthorizedRequest",
        dispatch: dispatch_authorized,
    }
}

inventory::submit! {
    RouteDispatch {
        handler_type: "ProtocolBindingRequest",
        dispatch: dispatch_binding,
    }
}

inventory::submit! {
    RouteDispatch {
        handler_type: "ProtocolFailingRequest",
        dispatch: dispatch_failing,
    }
}

inventory::submit! {
    RouteDispatch {
        handler_type: "ProtocolPublicRequest",
        dispatch: dispatch_public,
    }
}

inventory::submit! {
    RouteEntry::new(
        HttpMethod::Get,
        AUTHZ_PATH,
        "ProtocolAuthorizedRequest",
        "String",
        TextRef::text("Protocol authorized endpoint"),
        TextRef::text("test-only endpoint guarded by #[authorize]"),
        TextRef::text("protocol"),
        &[],
        AUTHENTICATED_ONLY,
    )
}

inventory::submit! {
    RouteEntry::new(
        HttpMethod::Get,
        BINDING_PATH,
        "ProtocolBindingRequest",
        "String",
        TextRef::text("Protocol binding endpoint"),
        TextRef::text("test-only endpoint with a required IdentityUser binding"),
        TextRef::text("protocol"),
        &[],
        &[],
    )
}

inventory::submit! {
    RouteEntry::new(
        HttpMethod::Get,
        FAILING_PATH,
        "ProtocolFailingRequest",
        "String",
        TextRef::text("Protocol failing endpoint"),
        TextRef::text("test-only endpoint that fails with an internal error"),
        TextRef::text("protocol"),
        &[],
        &[],
    )
}

inventory::submit! {
    RouteEntry::new(
        HttpMethod::Get,
        PUBLIC_PATH,
        "ProtocolPublicRequest",
        "String",
        TextRef::text("Protocol public endpoint"),
        TextRef::text("test-only public endpoint with no authorization rules"),
        TextRef::text("protocol"),
        &[],
        &[],
    )
}

// ---------------------------------------------------------------------------
// Host setup + helpers
// ---------------------------------------------------------------------------

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

async fn spawn_auth_host(port: u16) {
    let addr = format!("127.0.0.1:{}", port);
    let host = webx_host::server::Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .add_authentication()
        .configure(|b| {
            b.useOptions(|o| {
                o.jwt.secret = String::from_utf8_lossy(TEST_SECRET).to_string();
            });
        })
        .build();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(500)).await;
}

/// Case-insensitive header lookup (the wire spelling of `WWW-Authenticate`
/// depends on the HTTP stack, so assertions must not be keyed on case).
fn header_value<'a>(headers: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.as_str().eq_ignore_ascii_case(name))
        .and_then(|(_, value)| value.to_str().ok())
}

fn challenge_of(headers: &reqwest::header::HeaderMap) -> String {
    header_value(headers, "www-authenticate")
        .unwrap_or_else(|| panic!("401 must carry a WWW-Authenticate challenge"))
        .to_string()
}

fn create_valid_token() -> String {
    #[derive(serde::Serialize)]
    struct Claims {
        sub: String,
        roles: Vec<String>,
        permissions: Vec<String>,
        exp: usize,
    }

    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as usize + 3600)
        .unwrap();

    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &Claims {
            sub: "protocol-user".to_string(),
            roles: vec!["user".to_string()],
            permissions: Vec::new(),
            exp,
        },
        &jsonwebtoken::EncodingKey::from_secret(TEST_SECRET),
    )
    .unwrap()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// `#[authorize]` route, no credentials → 401 through the single writer.
#[tokio::test]
async fn protected_endpoint_without_token_returns_401_with_challenge() {
    let port = find_free_port();
    spawn_auth_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}{}", port, AUTHZ_PATH))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 401);

    let content_type = header_value(resp.headers(), "content-type").unwrap_or_default();
    assert!(
        content_type.contains("application/problem+json"),
        "expected application/problem+json, got {content_type}"
    );

    let challenge = challenge_of(resp.headers());
    assert!(
        challenge.to_ascii_lowercase().contains("bearer"),
        "WWW-Authenticate must advertise the Bearer scheme, got {challenge}"
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 401);
    assert_eq!(body["title"], "Unauthorized");
}

/// Binding-layer 401 (required `#[fromclaim] user: IdentityUser`, no
/// `#[authorize]`): the endpoint returns `Err(Unauthorized)` and the host writes
/// it — same writer, same challenge.
#[tokio::test]
async fn binding_layer_401_carries_same_challenge() {
    let port = find_free_port();
    spawn_auth_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}{}", port, BINDING_PATH))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 401);

    let content_type = header_value(resp.headers(), "content-type").unwrap_or_default();
    assert!(
        content_type.contains("application/problem+json"),
        "expected application/problem+json, got {content_type}"
    );

    let challenge = challenge_of(resp.headers());
    assert!(
        challenge.to_ascii_lowercase().contains("bearer"),
        "binding-layer 401 must carry the Bearer challenge too, got {challenge}"
    );
}

/// Invalid JWT → 401, and the body must not expose the decoder's error text
/// (`InvalidToken`, `invalid or expired token: <reason>`, ...).
#[tokio::test]
async fn invalid_token_401_does_not_leak_decoder_details() {
    let port = find_free_port();
    spawn_auth_host(port).await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}{}", port, AUTHZ_PATH))
        .header("authorization", "Bearer definitely-not-a-jwt")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 401);

    let challenge = challenge_of(resp.headers());
    assert!(
        challenge.to_ascii_lowercase().contains("bearer"),
        "WWW-Authenticate must advertise the Bearer scheme, got {challenge}"
    );

    let raw = resp.text().await.unwrap();
    let lowered = raw.to_ascii_lowercase();
    assert!(
        !lowered.contains("invalidtoken"),
        "401 body leaked the decoder error kind: {raw}"
    );
    assert!(
        !lowered.contains("invalid or expired token:"),
        "401 body leaked the decoder reason after the static detail: {raw}"
    );
    assert!(
        !lowered.contains("base64") && !lowered.contains("json error"),
        "401 body leaked decoding internals: {raw}"
    );

    let body: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(body["status"], 401);
}

/// Sanity check: the 401s above are caused by the missing/invalid credentials,
/// not by the route being broken — a valid token reaches the handler.
#[tokio::test]
async fn valid_token_reaches_protected_endpoint() {
    let port = find_free_port();
    spawn_auth_host(port).await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}{}", port, AUTHZ_PATH))
        .header("authorization", format!("Bearer {}", create_valid_token()))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.text().await.unwrap(), "\"authorized\"");
}

// ── #18: a bad token means "anonymous", not "reject the request" ──
//
// A client that still holds an expired token must be able to call public
// endpoints (login, health, docs); authentication is a downgrade to anonymous,
// and only the endpoint's own authorization turns that into a 401.

/// Public endpoint + invalid token → 200 (the token is ignored).
#[tokio::test]
async fn public_endpoint_ignores_an_invalid_token() {
    let port = find_free_port();
    spawn_auth_host(port).await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}{}", port, PUBLIC_PATH))
        .header("authorization", "Bearer definitely-not-a-jwt")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status().as_u16(),
        200,
        "an invalid token must not lock a public endpoint (e.g. login) out"
    );
    assert_eq!(resp.text().await.unwrap(), "\"public\"");
}

/// Protected endpoint + invalid token → 401 with the challenge: the endpoint's
/// `#[authorize]` rule sees no principal and the single unauthorized writer
/// answers, exactly as for a request that never sent a token.
#[tokio::test]
async fn protected_endpoint_with_invalid_token_is_401_with_challenge() {
    let port = find_free_port();
    spawn_auth_host(port).await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}{}", port, AUTHZ_PATH))
        .header("authorization", "Bearer definitely-not-a-jwt")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 401);
    let challenge = challenge_of(resp.headers());
    assert!(
        challenge.to_ascii_lowercase().contains("bearer"),
        "got {challenge}"
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 401);
    assert_eq!(body["title"], "Unauthorized");
    assert_eq!(body["detail"], "authentication required");
}

/// 5xx responses are logged, not echoed: the internal message stays server-side.
#[tokio::test]
async fn internal_error_does_not_leak_message() {
    let port = find_free_port();
    spawn_auth_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}{}", port, FAILING_PATH))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 500);

    let raw = resp.text().await.unwrap();
    assert!(
        !raw.contains("secret.db") && !raw.contains("/var/lib/app"),
        "5xx body leaked the internal failure message: {raw}"
    );

    let body: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(body["status"], 500);
}
