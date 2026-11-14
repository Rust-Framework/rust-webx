//! WEBX-02: asynchronous health checks.
//!
//! Covers async `pass`/`warn`/`fail`, per-check timeout → `fail`, the sync view
//! marking async checks as `warn` (1.3 compatibility), and the readiness
//! endpoint answering 503/200 from an async probe.

mod test_utils;

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use webx_core::mode::AppMode;
use webx_core::routing::IEndpoint;
use webx_host::health::{
    build_health_response_async, HealthCheckEndpoint, HealthCheckRegistry, HealthStatus,
    IHealthCheck,
};
use webx_host::server::Host;

struct Pass;

#[async_trait::async_trait]
impl IHealthCheck for Pass {
    fn name(&self) -> &str {
        "pass"
    }
    async fn check(&self) -> HealthStatus {
        HealthStatus::pass()
    }
}

struct Warn;

#[async_trait::async_trait]
impl IHealthCheck for Warn {
    fn name(&self) -> &str {
        "warn"
    }
    async fn check(&self) -> HealthStatus {
        HealthStatus::warn("degraded")
    }
}

struct Fail;

#[async_trait::async_trait]
impl IHealthCheck for Fail {
    fn name(&self) -> &str {
        "fail"
    }
    async fn check(&self) -> HealthStatus {
        HealthStatus::fail("dependency down")
    }
}

struct Slow(Duration);

#[async_trait::async_trait]
impl IHealthCheck for Slow {
    fn name(&self) -> &str {
        "slow"
    }
    async fn check(&self) -> HealthStatus {
        tokio::time::sleep(self.0).await;
        HealthStatus::pass()
    }
}

struct Toggle(Arc<AtomicBool>);

#[async_trait::async_trait]
impl IHealthCheck for Toggle {
    fn name(&self) -> &str {
        "toggle"
    }
    async fn check(&self) -> HealthStatus {
        if self.0.load(Ordering::SeqCst) {
            HealthStatus::pass()
        } else {
            HealthStatus::fail("not ready")
        }
    }
}

#[tokio::test]
async fn async_statuses_are_reported_in_registration_order() {
    let registry = HealthCheckRegistry::new();
    registry.register_async(Arc::new(Pass), Duration::from_secs(1));
    registry.register_async(Arc::new(Warn), Duration::from_secs(1));
    registry.register_async(Arc::new(Fail), Duration::from_secs(1));

    let entries = registry.snapshot_async().await;
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, ["pass", "warn", "fail"]);
    assert_eq!(entries[0].status, "pass");
    assert_eq!(entries[1].status, "warn");
    assert_eq!(entries[2].status, "fail");
    assert_eq!(registry.overall_status_async().await, "fail");
    assert!(!registry.all_healthy_async().await);
}

#[tokio::test]
async fn warn_does_not_fail_readiness() {
    let registry = HealthCheckRegistry::new();
    registry.register_async(Arc::new(Pass), Duration::from_secs(1));
    registry.register_async(Arc::new(Warn), Duration::from_secs(1));

    assert_eq!(registry.overall_status_async().await, "warn");
    assert!(registry.all_healthy_async().await, "warn must not fail");

    let body: serde_json::Value =
        serde_json::from_slice(&build_health_response_async(&registry).await)
            .expect("the health payload is JSON");
    assert_eq!(body["status"], "warn");
    assert_eq!(body["checks"].as_array().map(Vec::len), Some(2));
}

#[tokio::test]
async fn timeout_is_reported_as_fail_with_timed_out_detail() {
    let registry = HealthCheckRegistry::new();
    registry.register_async(
        Arc::new(Slow(Duration::from_millis(200))),
        Duration::from_millis(20),
    );
    registry.register_async(Arc::new(Pass), Duration::from_secs(1));

    let entries = registry.snapshot_async().await;
    assert_eq!(entries[0].status, "fail");
    let detail = entries[0].detail.as_deref().unwrap_or_default();
    assert!(
        detail.contains("timed out"),
        "the timeout must be visible to operators: {detail}"
    );
    assert!(
        detail.contains("20ms"),
        "the detail names the configured timeout: {detail}"
    );
    assert_eq!(
        entries[1].status, "pass",
        "one timed-out check must not poison the others"
    );
    assert_eq!(registry.overall_status_async().await, "fail");
    assert!(!registry.all_healthy_async().await);
}

/// The 1.3 sync API keeps working: it evaluates sync checks and reports each
/// async check as `warn` (never `fail`, never an error).
#[test]
fn sync_view_marks_async_checks_as_warn() {
    let registry = HealthCheckRegistry::new();
    registry.register("db", Arc::new(HealthStatus::pass));
    registry.register_async(Arc::new(Pass), Duration::from_secs(1));

    let entries = registry.snapshot();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].status, "pass");
    assert_eq!(entries[1].name, "pass");
    assert_eq!(entries[1].status, "warn");
    assert!(
        entries[1]
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("requires async evaluation"),
        "the detail explains why the async check was not evaluated: {:?}",
        entries[1].detail
    );
    assert_eq!(registry.overall_status(), "warn");
    assert!(
        registry.all_healthy(),
        "`warn` must not fail the legacy view"
    );
}

#[tokio::test]
async fn readiness_endpoint_returns_503_for_async_fail() {
    let registry = HealthCheckRegistry::new();
    registry.register_async(Arc::new(Fail), Duration::from_secs(1));

    let mut ctx = test_utils::TestHttpContext::new("GET", "/health/ready");
    HealthCheckEndpoint::new(registry)
        .handle(&mut ctx)
        .await
        .expect("the endpoint writes a response");

    let (status, headers, body) = ctx.into_response_parts();
    assert_eq!(status, 503);
    assert!(
        headers
            .iter()
            .any(|(name, value)| name == "content-type" && value == "application/health+json"),
        "RFC 8407 content type: {headers:?}"
    );
    let json: serde_json::Value =
        serde_json::from_slice(&body.expect("the endpoint writes a body")).unwrap();
    assert_eq!(json["status"], "fail");
    assert_eq!(json["checks"][0]["status"], "fail");
}

#[tokio::test]
async fn readiness_endpoint_returns_200_for_async_pass() {
    let registry = HealthCheckRegistry::new();
    registry.register_async(Arc::new(Pass), Duration::from_secs(1));

    let mut ctx = test_utils::TestHttpContext::new("GET", "/health/ready");
    HealthCheckEndpoint::new(registry)
        .handle(&mut ctx)
        .await
        .expect("the endpoint writes a response");

    let (status, _, body) = ctx.into_response_parts();
    assert_eq!(status, 200);
    let json: serde_json::Value =
        serde_json::from_slice(&body.expect("the endpoint writes a body")).unwrap();
    assert_eq!(json["status"], "pass");
}

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local_addr")
        .port()
}

/// End-to-end: `HostBuilder::add_health_check_async` reaches `/health/ready`
/// and flips the status code as the probe changes.
#[tokio::test]
async fn host_builder_wires_async_checks_into_the_readiness_endpoint() {
    let healthy = Arc::new(AtomicBool::new(false));
    let port = find_free_port();
    let addr = format!("127.0.0.1:{port}");
    let base = format!("http://{addr}");

    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_health_check_async(Toggle(Arc::clone(&healthy)))
        .try_build()
        .expect("host must build");

    let server = tokio::spawn(async move {
        let _ = host.run_at(&addr).await;
    });

    // /health/live is static, so it becomes ready independently of the probe.
    let mut started = false;
    for _ in 0..60 {
        if let Ok(response) = reqwest::get(format!("{base}/health/live")).await {
            if response.status().is_success() {
                started = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(started, "the server did not start at {base}");

    let response = reqwest::get(format!("{base}/health/ready"))
        .await
        .expect("readiness request");
    assert_eq!(response.status().as_u16(), 503);
    let body: serde_json::Value = response.json().await.expect("health JSON");
    assert_eq!(body["status"], "fail");

    healthy.store(true, Ordering::SeqCst);
    let response = reqwest::get(format!("{base}/health/ready"))
        .await
        .expect("readiness request");
    assert_eq!(response.status().as_u16(), 200);
    let body: serde_json::Value = response.json().await.expect("health JSON");
    assert_eq!(body["status"], "pass");

    server.abort();
}
