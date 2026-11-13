//! Middleware slot semantics (#4).
//!
//! A middleware registered with the default `use_middleware` runs *after*
//! authentication, so it can read the principal; the framework's own
//! `RequestTracing` was stuck before JWT and always logged `anon`. The explicit
//! `BeforeCors` slot runs before CORS, which is visible because a short-circuit
//! there prevents the CORS headers from being added.

#![allow(clippy::type_complexity)]

use std::net::TcpListener;
use std::time::Duration;

use std::ops::ControlFlow;
use webx_core::action::{JsonResult, Response};
use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::localization::TextRef;
use webx_core::middleware::{IMiddleware, MiddlewareSlot};
use webx_core::route::scan::{HandlerRegistration, RouteDispatch, RouteEntry};
use webx_core::routing::HttpMethod;

const WHOAMI_PATH: &str = "/test/slots/whoami";
const BLOCKED_PATH: &str = "/test/slots/blocked";
const TEST_SECRET: &[u8] = b"middleware-slot-test-secret-32-bytes-min!!";

type DispatchFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Response>> + Send + 'a>>;

fn dispatch_ok(_ctx: &mut dyn IHttpContext) -> DispatchFuture<'_> {
    Box::pin(async { Ok(Box::new(JsonResult::new("ok")) as Response) })
}

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

macro_rules! register_route {
    ($type_id:ty, $name:literal, $path:expr) => {
        inventory::submit! {
            HandlerRegistration {
                req_type_id: std::any::TypeId::of::<$type_id>(),
                req_type_name: $name,
                factory: noop_factory,
                call: noop_call,
            }
        }
        inventory::submit! {
            RouteDispatch {
                handler_type: $name,
                dispatch: dispatch_ok,
            }
        }
        inventory::submit! {
            RouteEntry::new(
                HttpMethod::Get,
                $path,
                $name,
                "String",
                TextRef::text("middleware slot test"),
                TextRef::text("test-only endpoint"),
                TextRef::text("slots"),
                &[],
                &[],
            )
        }
    };
}

register_route!(u8, "SlotWhoamiRequest", WHOAMI_PATH);
register_route!(u16, "SlotBlockedRequest", BLOCKED_PATH);

/// Default slot: records whether authentication already ran.
#[derive(Default)]
struct IdentityProbe;

#[async_trait::async_trait]
impl IMiddleware for IdentityProbe {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        let saw = ctx.claims().is_some();
        ctx.response_mut()
            .set_header("x-saw-identity", if saw { "yes" } else { "no" });
        Ok(ControlFlow::Continue(()))
    }
}

/// BeforeCors slot: short-circuits the blocked path, proving CORS never ran.
#[derive(Default)]
struct EarlyBlocker;

#[async_trait::async_trait]
impl IMiddleware for EarlyBlocker {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        if ctx.request().path() == BLOCKED_PATH {
            ctx.response_mut().set_status(418);
            ctx.response_mut().set_header("x-early", "blocked");
            return Ok(ControlFlow::Break(()));
        }
        Ok(ControlFlow::Continue(()))
    }
}

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn header<'a>(headers: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.as_str().eq_ignore_ascii_case(name))
        .and_then(|(_, value)| value.to_str().ok())
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
            sub: "slot-user".to_string(),
            roles: vec!["user".to_string()],
            permissions: Vec::new(),
            exp,
        },
        &jsonwebtoken::EncodingKey::from_secret(TEST_SECRET),
    )
    .unwrap()
}

async fn spawn_host() -> (String, webx_host::server::ServerHandle) {
    let port = find_free_port();
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
        .use_cors(webx_host::cors::CorsConfig {
            origins: vec!["https://app.example.com".to_string()],
            ..Default::default()
        })
        .use_middleware::<IdentityProbe>()
        .use_middleware_at::<EarlyBlocker>(MiddlewareSlot::BeforeCors)
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    let base = format!("http://127.0.0.1:{}", port);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(resp) = reqwest::get(format!("{base}/health/live")).await {
            if resp.status().as_u16() == 200 {
                break;
            }
        }
        assert!(std::time::Instant::now() < deadline, "server not ready");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    (base, handle)
}

/// The default slot runs after JWT, so a plain `use_middleware` sees the user.
#[tokio::test]
async fn default_slot_middleware_sees_the_authenticated_identity() {
    let (base, handle) = spawn_host().await;

    let resp = reqwest::Client::new()
        .get(format!("{base}{WHOAMI_PATH}"))
        .header("authorization", format!("Bearer {}", create_valid_token()))
        .header("origin", "https://app.example.com")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        header(resp.headers(), "x-saw-identity"),
        Some("yes"),
        "default-slot middleware must run after authentication"
    );
    assert_eq!(
        header(resp.headers(), "access-control-allow-origin"),
        Some("https://app.example.com")
    );

    handle.shutdown();
}

/// Anonymous requests still reach the middleware — it just sees no identity.
#[tokio::test]
async fn default_slot_middleware_runs_for_anonymous_requests() {
    let (base, handle) = spawn_host().await;

    let resp = reqwest::get(format!("{base}{WHOAMI_PATH}")).await.unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(header(resp.headers(), "x-saw-identity"), Some("no"));

    handle.shutdown();
}

/// BeforeCors really runs before CORS: a short-circuit there means the CORS
/// middleware never added its headers.
#[tokio::test]
async fn before_cors_slot_short_circuits_before_cors_headers() {
    let (base, handle) = spawn_host().await;

    let resp = reqwest::Client::new()
        .get(format!("{base}{BLOCKED_PATH}"))
        .header("origin", "https://app.example.com")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 418);
    assert_eq!(header(resp.headers(), "x-early"), Some("blocked"));
    assert_eq!(
        header(resp.headers(), "access-control-allow-origin"),
        None,
        "the BeforeCors middleware must run before CORS"
    );

    handle.shutdown();
}
