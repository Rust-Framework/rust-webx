//! RFC 9110 method semantics: `Allow` on 405, OPTIONS discovery, CORS preflight.
//!
//! Regressions covered from the 0.9.0 review (#19):
//! * a 405 from a path/method mismatch must carry `Allow`;
//! * a plain `OPTIONS` (not a CORS preflight) reports the path's methods or 404s;
//! * only a real preflight (`Origin` + `Access-Control-Request-Method`) is
//!   short-circuited by CORS.

#![allow(clippy::type_complexity)]

use std::net::TcpListener;
use std::time::Duration;

use webx_core::action::{JsonResult, Response};
use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::localization::TextRef;
use webx_core::route::scan::{HandlerRegistration, RouteDispatch, RouteEntry};
use webx_core::routing::HttpMethod;

const RESOURCE_PATH: &str = "/test/methods/resource";

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
    ($type_id:ty, $name:literal, $method:ident) => {
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
                HttpMethod::$method,
                RESOURCE_PATH,
                $name,
                "String",
                TextRef::text("method test endpoint"),
                TextRef::text("test-only endpoint"),
                TextRef::text("methods"),
                &[],
                &[],
            )
        }
    };
}

register_route!(u8, "MethodGetRequest", Get);
register_route!(u16, "MethodPutRequest", Put);

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

async fn spawn_host() -> (String, webx_host::server::ServerHandle) {
    let port = find_free_port();
    let addr = format!("127.0.0.1:{}", port);
    let host = webx_host::server::Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .use_cors(webx_host::cors::CorsConfig {
            origins: vec!["https://app.example.com".to_string()],
            ..Default::default()
        })
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

/// 405 must name the methods the path allows.
#[tokio::test]
async fn method_not_allowed_carries_allow_header() {
    let (base, handle) = spawn_host().await;

    let resp = reqwest::Client::new()
        .post(format!("{base}{RESOURCE_PATH}"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 405);
    let allow = header(resp.headers(), "allow").expect("405 must carry Allow");
    let allow = allow.to_ascii_uppercase();
    for expected in ["GET", "PUT", "HEAD"] {
        assert!(
            allow.contains(expected),
            "Allow={allow:?} missing {expected}"
        );
    }
    assert!(header(resp.headers(), "content-type")
        .unwrap_or_default()
        .contains("application/problem+json"));

    handle.shutdown();
}

/// A plain OPTIONS (no CORS request headers) reports the path's methods.
#[tokio::test]
async fn options_reports_allowed_methods_for_an_existing_path() {
    let (base, handle) = spawn_host().await;

    let resp = reqwest::Client::new()
        .request(reqwest::Method::OPTIONS, format!("{base}{RESOURCE_PATH}"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 204);
    let allow = header(resp.headers(), "allow").expect("OPTIONS must carry Allow");
    assert!(
        allow.contains("GET") && allow.contains("PUT"),
        "Allow={allow:?}"
    );

    handle.shutdown();
}

/// A plain OPTIONS for an unknown path is a 404, not a fabricated 204.
#[tokio::test]
async fn options_for_an_unknown_path_is_404() {
    let (base, handle) = spawn_host().await;

    let resp = reqwest::Client::new()
        .request(
            reqwest::Method::OPTIONS,
            format!("{base}/test/methods/does-not-exist"),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 404);

    handle.shutdown();
}

/// A real CORS preflight is answered by the CORS middleware (even before
/// routing); it must not become a 404.
#[tokio::test]
async fn cors_preflight_is_answered_not_routed() {
    let (base, handle) = spawn_host().await;

    let resp = reqwest::Client::new()
        .request(reqwest::Method::OPTIONS, format!("{base}/anything/at/all"))
        .header("origin", "https://app.example.com")
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 204);
    assert_eq!(
        header(resp.headers(), "access-control-allow-origin"),
        Some("https://app.example.com")
    );
    assert!(header(resp.headers(), "access-control-allow-methods")
        .unwrap_or_default()
        .contains("POST"));

    handle.shutdown();
}

/// Sanity: the registered GET still works and gets CORS headers.
#[tokio::test]
async fn allowed_method_still_succeeds() {
    let (base, handle) = spawn_host().await;

    let resp = reqwest::Client::new()
        .get(format!("{base}{RESOURCE_PATH}"))
        .header("origin", "https://app.example.com")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        header(resp.headers(), "access-control-allow-origin"),
        Some("https://app.example.com")
    );

    handle.shutdown();
}
