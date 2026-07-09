//! Health check registry tests.
//!
//! Tests the HealthCheckRegistry API: snapshot, overall_status, and
//! the build_health_response helper for RFC 8407 compliance.

use rust_webx_host::health::{HealthCheckRegistry, HealthStatus};
use std::sync::Arc;

#[test]
fn registry_snapshot_returns_registered_checks() {
    let registry = HealthCheckRegistry::new();
    registry.register("db", Arc::new(HealthStatus::pass));
    registry.register("cache", Arc::new(|| HealthStatus::fail("down")));

    let snap = registry.snapshot();
    assert_eq!(snap.len(), 2);
    assert_eq!(snap[0].name, "db");
    assert_eq!(snap[0].status, "pass");
    assert!(snap[0].detail.is_none());
    assert_eq!(snap[1].name, "cache");
    assert_eq!(snap[1].status, "fail");
    assert_eq!(snap[1].detail.as_deref(), Some("down"));
}

#[test]
fn registry_overall_status_fail_dominates() {
    let registry = HealthCheckRegistry::new();
    registry.register("db", Arc::new(HealthStatus::pass));
    registry.register("cache", Arc::new(|| HealthStatus::fail("down")));

    assert_eq!(registry.overall_status(), "fail");
}

#[test]
fn registry_overall_status_warn_when_no_fail() {
    let registry = HealthCheckRegistry::new();
    registry.register("db", Arc::new(HealthStatus::pass));
    registry.register("cache", Arc::new(|| HealthStatus::warn("degraded")));

    assert_eq!(registry.overall_status(), "warn");
}

#[test]
fn registry_empty_returns_pass() {
    let registry = HealthCheckRegistry::new();
    assert_eq!(registry.overall_status(), "pass");
    assert!(registry.snapshot().is_empty());
}

#[test]
fn registry_all_pass_returns_pass() {
    let registry = HealthCheckRegistry::new();
    registry.register("db", Arc::new(HealthStatus::pass));
    registry.register("cache", Arc::new(HealthStatus::pass));

    assert_eq!(registry.overall_status(), "pass");
    assert!(registry.all_healthy());
}

#[test]
fn registry_all_healthy_false_when_any_fail() {
    let registry = HealthCheckRegistry::new();
    registry.register("db", Arc::new(HealthStatus::pass));
    registry.register("cache", Arc::new(|| HealthStatus::fail("down")));

    assert!(!registry.all_healthy());
}

#[test]
fn health_http_status_maps_fail_to_503() {
    assert_eq!(rust_webx_host::health::health_http_status("fail"), 503);
    assert_eq!(rust_webx_host::health::health_http_status("pass"), 200);
    assert_eq!(rust_webx_host::health::health_http_status("warn"), 200);
}

#[test]
fn build_health_response_empty_registry() {
    let registry = HealthCheckRegistry::new();
    let body: serde_json::Value =
        serde_json::from_slice(&rust_webx_host::health::build_health_response(&registry))
            .unwrap();
    assert_eq!(body["status"], "pass");
}

#[test]
fn registry_snapshot_includes_warn_detail() {
    let registry = HealthCheckRegistry::new();
    registry.register(
        "queue",
        Arc::new(|| HealthStatus::warn("latency 500ms")),
    );

    let snap = registry.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].status, "warn");
    assert_eq!(snap[0].detail.as_deref(), Some("latency 500ms"));
}
