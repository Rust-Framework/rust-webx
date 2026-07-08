//! Integration tests for LRWF host.
//!
//! These tests spin up a minimal LRWF host and verify full HTTP cycles.

use std::net::TcpListener;
use std::sync::Arc;

async fn spawn_test_host(port: u16) {
    spawn_test_host_with(port, |b| b).await;
}

async fn spawn_test_host_with<F>(port: u16, configure: F)
where
    F: FnOnce(rust_webx_host::server::HostBuilder) -> rust_webx_host::server::HostBuilder,
{
    let addr = format!("127.0.0.1:{}", port);
    let builder = rust_webx_host::server::Host::builder()
        .mode(rust_webx_core::mode::AppMode::Development)
        .no_spa();
    let host = configure(builder).build();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
}

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

// ---------------------------------------------------------------------------
// 404 routing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn integration_404_for_unregistered_route() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{}/nonexistent", port))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 404);
    assert_eq!(body["title"], "Not Found");
    assert!(body["detail"].as_str().unwrap().contains("Not Found"));
}

#[tokio::test]
async fn integration_404_returns_problem_json() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/nope", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 404);
    let content_type = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        content_type.contains("application/problem+json"),
        "expected application/problem+json, got {}",
        content_type
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 404);
    assert_eq!(body["title"], "Not Found");
}

// ---------------------------------------------------------------------------
// OpenAPI endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn integration_health_check_openapi_available() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{}/api/openapi.html", port))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let content_type = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("text/html"));
}

#[tokio::test]
async fn integration_openapi_json_available() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{}/api/openapi.json", port))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("openapi").is_some());
    assert!(body.get("info").is_some());
}

// ---------------------------------------------------------------------------
// Health endpoints (RFC 8407)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn integration_health_returns_pass_with_no_probes() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let content_type = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        content_type.contains("application/health+json"),
        "expected application/health+json, got {}",
        content_type
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "pass");
    assert!(body.get("checks").is_none(), "empty registry should omit checks");
}

#[tokio::test]
async fn integration_healthz_alias_matches_health() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let client = reqwest::Client::new();
    let health_resp = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .unwrap();
    let healthz_resp = client
        .get(format!("http://127.0.0.1:{}/healthz", port))
        .send()
        .await
        .unwrap();

    assert_eq!(health_resp.status(), healthz_resp.status());
    let health_body: serde_json::Value = health_resp.json().await.unwrap();
    let healthz_body: serde_json::Value = healthz_resp.json().await.unwrap();
    assert_eq!(health_body, healthz_body);
}

#[tokio::test]
async fn integration_health_live_returns_pass() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health/live", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "pass");
}

#[tokio::test]
async fn integration_health_ready_matches_health() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let client = reqwest::Client::new();
    let health_resp = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .unwrap();
    let ready_resp = client
        .get(format!("http://127.0.0.1:{}/health/ready", port))
        .send()
        .await
        .unwrap();

    assert_eq!(health_resp.status(), ready_resp.status());
    let health_body: serde_json::Value = health_resp.json().await.unwrap();
    let ready_body: serde_json::Value = ready_resp.json().await.unwrap();
    assert_eq!(health_body, ready_body);
}

#[tokio::test]
async fn integration_health_with_failing_probe_returns_fail() {
    let port = find_free_port();
    spawn_test_host_with(port, |b| {
        b.add_health_check("db", || {
            rust_webx_host::health::HealthStatus::fail("db unreachable")
        })
    })
    .await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 503);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "fail");
    let checks = body["checks"].as_array().expect("checks array present");
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0]["name"], "db");
    assert_eq!(checks[0]["status"], "fail");
    assert_eq!(checks[0]["detail"], "db unreachable");
}

#[tokio::test]
async fn integration_health_probe_evaluated_at_request_time() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let healthy = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&healthy);
    let port = find_free_port();
    spawn_test_host_with(port, move |b| {
        b.add_health_check("db", move || {
            if flag.load(Ordering::Relaxed) {
                rust_webx_host::health::HealthStatus::pass()
            } else {
                rust_webx_host::health::HealthStatus::fail("db down")
            }
        })
    })
    .await;

    let url = format!("http://127.0.0.1:{}/health", port);
    let ok = reqwest::get(&url).await.unwrap();
    assert_eq!(ok.status().as_u16(), 200);

    healthy.store(false, Ordering::Relaxed);
    let fail = reqwest::get(&url).await.unwrap();
    assert_eq!(fail.status().as_u16(), 503);
    let body: serde_json::Value = fail.json().await.unwrap();
    assert_eq!(body["status"], "fail");
}

// ---------------------------------------------------------------------------
// CORS preflight
// ---------------------------------------------------------------------------

#[tokio::test]
async fn integration_cors_preflight_returns_204() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let client = reqwest::Client::new();
    let resp = client
        .request(
            reqwest::Method::OPTIONS,
            format!("http://127.0.0.1:{}/api/openapi.json", port),
        )
        .header("origin", "https://example.com")
        .header("access-control-request-method", "GET")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 204);
    let headers = resp.headers();
    assert!(
        headers.contains_key("access-control-allow-origin"),
        "missing access-control-allow-origin"
    );
    assert!(
        headers.contains_key("access-control-allow-methods"),
        "missing access-control-allow-methods"
    );
    assert!(
        headers.contains_key("access-control-allow-headers"),
        "missing access-control-allow-headers"
    );
}

#[tokio::test]
async fn integration_cors_actual_request_has_headers() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/health", port))
        .header("origin", "https://example.com")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    assert!(
        resp.headers().contains_key("access-control-allow-origin"),
        "missing access-control-allow-origin on actual request"
    );
}

// ---------------------------------------------------------------------------
// Default security & observability middleware
// ---------------------------------------------------------------------------

#[tokio::test]
async fn integration_default_security_headers_present() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let headers = resp.headers();
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(headers.get("x-frame-options").unwrap(), "DENY");
    assert_eq!(
        headers.get("referrer-policy").unwrap(),
        "strict-origin-when-cross-origin"
    );
}

#[tokio::test]
async fn integration_default_request_id_present() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let request_id = resp
        .headers()
        .get("x-request-id")
        .expect("x-request-id header present by default");
    assert!(!request_id.is_empty());
}

// ---------------------------------------------------------------------------
// use_middleware_with API + RateLimit short-circuit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn integration_rate_limit_returns_429_when_exceeded() {
    let port = find_free_port();
    spawn_test_host_with(port, |b| {
        b.use_middleware_with(|| {
            Arc::new(rust_webx_host::rate_limit::RateLimitMiddleware::new(1.0, 2))
                as Arc<dyn rust_webx_core::middleware::IMiddleware>
        })
    })
    .await;

    let client = reqwest::Client::new();
    // First 2 requests: allowed (burst=2)
    let r1 = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .unwrap();
    let r2 = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .unwrap();
    assert_eq!(r1.status().as_u16(), 200);
    assert_eq!(r2.status().as_u16(), 200);

    // Third request immediately: should be rate-limited (429)
    let r3 = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .unwrap();
    assert_eq!(r3.status().as_u16(), 429);
    let ct = r3
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.contains("application/problem+json"));
    let body: serde_json::Value = r3.json().await.unwrap();
    assert_eq!(body["status"], 429);
    assert_eq!(body["title"], "Too Many Requests");
}

#[tokio::test]
async fn integration_use_middleware_with_runs_in_pipeline() {
    use rust_webx_core::http::IHttpContext;
    use rust_webx_core::middleware::IMiddleware;
    use std::ops::ControlFlow;

    struct HeaderTagMiddleware;
    #[async_trait::async_trait]
    impl IMiddleware for HeaderTagMiddleware {
        async fn invoke(
            &self,
            ctx: &mut dyn IHttpContext,
        ) -> rust_webx_core::error::Result<ControlFlow<()>> {
            ctx.response_mut().set_header("x-tagged", "true");
            Ok(ControlFlow::Continue(()))
        }
    }

    let port = find_free_port();
    spawn_test_host_with(port, |b| {
        b.use_middleware_with(|| Arc::new(HeaderTagMiddleware) as Arc<dyn IMiddleware>)
    })
    .await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.headers().get("x-tagged").unwrap(), "true");
}

// ---------------------------------------------------------------------------
// Compression middleware
// ---------------------------------------------------------------------------

#[tokio::test]
async fn integration_compression_gzips_large_response() {
    use rust_webx_host::compression::{CompressionConfig, CompressionMiddleware};

    let port = find_free_port();
    spawn_test_host_with(port, |b| {
        b.use_middleware_with(|| {
            Arc::new(CompressionMiddleware::with_config(
                CompressionConfig::default().min_size(10),
            )) as Arc<dyn rust_webx_core::middleware::IMiddleware>
        })
    })
    .await;

    // Disable reqwest's auto-decompression so we can verify content-encoding header.
    let client = reqwest::Client::builder()
        .no_gzip()
        .build()
        .unwrap();

    let resp = client
        .get(format!("http://127.0.0.1:{}/api/openapi.json", port))
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        resp.headers().get("content-encoding").unwrap(),
        "gzip",
        "large response should be gzipped"
    );

    // Manually decompress to verify the body is valid JSON.
    use std::io::Read;
    let compressed = resp.bytes().await.unwrap();
    let mut decoder = flate2::read::GzDecoder::new(&compressed[..]);
    let mut decompressed = String::new();
    decoder.read_to_string(&mut decompressed).unwrap();
    let body: serde_json::Value = serde_json::from_str(&decompressed).unwrap();
    assert!(body.get("openapi").is_some());
}

#[tokio::test]
async fn integration_compression_skips_small_response() {
    let port = find_free_port();
    spawn_test_host_with(port, |b| {
        b.use_middleware::<rust_webx_host::compression::CompressionMiddleware>()
    })
    .await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/health/live", port))
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    assert!(
        resp.headers().get("content-encoding").is_none(),
        "small response should not be compressed"
    );
}

#[tokio::test]
async fn integration_compression_skips_without_accept_encoding() {
    let port = find_free_port();
    spawn_test_host_with(port, |b| {
        b.use_middleware::<rust_webx_host::compression::CompressionMiddleware>()
    })
    .await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/api/openapi.json", port))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    assert!(
        resp.headers().get("content-encoding").is_none(),
        "should not compress without accept-encoding"
    );
}
