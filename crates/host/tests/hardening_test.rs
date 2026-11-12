//! Regression tests for the 0.9.0 host hardening (docs/reviews/0.9.0-hardening.md).
//!
//! Covers four host defects from the adversarial review:
//! * #2 — a bind conflict must fail `run`/`run_at` instead of exiting silently.
//! * #3 — a shutdown requested before the accept loops start must not be lost.
//! * #8 — the SPA index fallback runs only after a router miss, only for HTML
//!   navigation, and never leaks its file cache policy into API responses.
//! * #10 — an oversized declared body still travels through the middleware
//!   pipeline (CORS/security headers/logging), instead of bypassing it.

#![allow(clippy::type_complexity)]

use std::net::TcpListener;
use std::time::Duration;

use webx_core::action::{JsonResult, Response};
use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::localization::TextRef;
use webx_core::route::scan::{HandlerRegistration, RouteDispatch, RouteEntry};
use webx_core::routing::HttpMethod;

const JSON_PATH: &str = "/test/hardening/json";

type DispatchFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Response>> + Send + 'a>>;

fn dispatch_json(_ctx: &mut dyn IHttpContext) -> DispatchFuture<'_> {
    Box::pin(async { Ok(Box::new(JsonResult::new(serde_json::json!({ "ok": true }))) as Response) })
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

inventory::submit! {
    HandlerRegistration {
        req_type_id: std::any::TypeId::of::<u64>(),
        req_type_name: "HardeningJsonRequest",
        factory: noop_factory,
        call: noop_call,
    }
}

inventory::submit! {
    RouteDispatch {
        handler_type: "HardeningJsonRequest",
        dispatch: dispatch_json,
    }
}

inventory::submit! {
    RouteEntry::new(
        HttpMethod::Get,
        JSON_PATH,
        "HardeningJsonRequest",
        "Json",
        TextRef::text("Hardening JSON endpoint"),
        TextRef::text("test-only endpoint returning JSON"),
        TextRef::text("hardening"),
        &[],
        &[],
    )
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

async fn wait_until_live(base: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(resp) = reqwest::get(format!("{base}/health/live")).await {
            if resp.status().as_u16() == 200 {
                return;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "server did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// #8: the SPA shell is served for a client-side route only when the client
/// asked for HTML; JSON clients and API paths get a real 404.
#[tokio::test]
async fn spa_fallback_is_post_routing_and_html_only() {
    let wwwroot = tempfile::tempdir().unwrap();
    std::fs::write(
        wwwroot.path().join("index.html"),
        b"<html><body>app shell</body></html>",
    )
    .unwrap();

    let port = find_free_port();
    let addr = format!("127.0.0.1:{}", port);
    let host = webx_host::server::Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .use_spa(wwwroot.path().to_path_buf())
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    let base = format!("http://127.0.0.1:{}", port);
    wait_until_live(&base).await;

    let client = reqwest::Client::new();

    // HTML navigation to a client-side route: the app shell.
    let html = client
        .get(format!("{base}/work/42"))
        .header("accept", "text/html,application/xhtml+xml")
        .send()
        .await
        .unwrap();
    assert_eq!(html.status().as_u16(), 200);
    assert!(header(html.headers(), "content-type")
        .unwrap_or_default()
        .contains("text/html"));
    assert_eq!(
        html.text().await.unwrap(),
        "<html><body>app shell</body></html>"
    );

    // A data client asking for the same path must get a 404, not 200 HTML.
    let json = client
        .get(format!("{base}/work/42"))
        .header("accept", "application/json")
        .send()
        .await
        .unwrap();
    assert_eq!(json.status().as_u16(), 404);
    assert!(header(json.headers(), "content-type")
        .unwrap_or_default()
        .contains("application/problem+json"));

    // API paths never fall back, even when the client accepts HTML.
    let api = client
        .get(format!("{base}/api/does-not-exist"))
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(api.status().as_u16(), 404);

    handle.shutdown();
}

/// #8: a JSON handler response must not inherit the SPA file cache policy.
#[tokio::test]
async fn api_json_response_does_not_inherit_spa_cache_policy() {
    let wwwroot = tempfile::tempdir().unwrap();
    std::fs::write(wwwroot.path().join("index.html"), b"shell").unwrap();

    let port = find_free_port();
    let addr = format!("127.0.0.1:{}", port);
    let host = webx_host::server::Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .use_spa(wwwroot.path().to_path_buf())
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    let base = format!("http://127.0.0.1:{}", port);
    wait_until_live(&base).await;

    let resp = reqwest::Client::new()
        .get(format!("{base}{JSON_PATH}"))
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let cache = header(resp.headers(), "cache-control").unwrap_or_default();
    assert!(
        !cache.contains("public"),
        "API JSON must not be publicly cacheable, got {cache:?}"
    );
    assert_eq!(resp.text().await.unwrap(), "{\"ok\":true}");

    handle.shutdown();
}

/// #10: an oversized declared body is rejected *inside* the pipeline, so the
/// CORS middleware still decorates the 413 response.
#[tokio::test]
async fn oversized_body_still_gets_cors_headers() {
    let port = find_free_port();
    let addr = format!("127.0.0.1:{}", port);
    let host = webx_host::server::Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .use_cors(webx_host::cors::CorsConfig {
            origins: vec!["https://app.example.com".to_string()],
            ..Default::default()
        })
        .configure(|app| {
            app.useOptions(|o| o.app.max_body_size = 64);
        })
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    let base = format!("http://127.0.0.1:{}", port);
    wait_until_live(&base).await;

    let resp = reqwest::Client::new()
        .post(format!("{base}/health"))
        .header("origin", "https://app.example.com")
        .body(vec![b'x'; 4096])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 413);
    assert_eq!(
        header(resp.headers(), "access-control-allow-origin"),
        Some("https://app.example.com"),
        "the 413 must pass through CORS like any other response"
    );
    assert!(header(resp.headers(), "content-type")
        .unwrap_or_default()
        .contains("application/problem+json"));

    handle.shutdown();
}

/// #2: a port conflict fails `run_at` instead of exiting successfully.
#[tokio::test]
async fn bind_conflict_returns_an_error() {
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("127.0.0.1:{}", occupied.local_addr().unwrap().port());

    let host = webx_host::server::Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .build();

    let result = host.run_at(&addr).await;
    assert!(
        result.is_err(),
        "binding an occupied port must surface as an error"
    );
    let message = format!("{}", result.unwrap_err());
    assert!(
        message.contains("failed to bind"),
        "unexpected error: {message}"
    );
}

/// #3: the documented `spawn(run); handle.shutdown()` pattern must not hang.
#[tokio::test]
async fn shutdown_before_accept_loop_starts_is_not_lost() {
    let port = find_free_port();
    let addr = format!("127.0.0.1:{}", port);
    let host = webx_host::server::Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .build();
    let handle = host.server_handle();

    let server = tokio::spawn(async move { host.run_at(&addr).await });
    // No sleep: the shutdown is requested before the accept loop can park on a
    // signal, which used to be dropped by `Notify::notify_waiters`.
    handle.shutdown();

    let joined = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("run_at must observe the early shutdown and return");
    joined.unwrap().unwrap();
}
