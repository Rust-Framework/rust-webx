//! Enhanced health check endpoints for Kubernetes.
//!
//! Provides HealthCheckRegistry and typed HealthStatus.
//! Endpoints follow RFC 8407 (`application/health+json` content type).
//!
//! Two registration styles coexist:
//!
//! * [`HealthCheckRegistry::register`] — a synchronous closure, evaluated on the
//!   calling thread (1.3 API; still supported).
//! * [`HealthCheckRegistry::register_async`] — an [`IHealthCheck`] port with its
//!   own timeout, evaluated concurrently by
//!   [`HealthCheckRegistry::snapshot_async`]. This is what probes a database, an
//!   HTTP dependency or a queue without blocking the runtime.

use futures_util::future::join_all;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::routing::IEndpoint;

/// The timeout a host uses for an async probe unless the caller passes one.
pub const DEFAULT_HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(3);

pub type HealthCheckFn = Arc<dyn Fn() -> HealthStatus + Send + Sync>;

/// Async health-check port.
///
/// Implement it for probes that must `.await` (database ping, HTTP dependency,
/// queue depth). Each check is evaluated by
/// [`HealthCheckRegistry::snapshot_async`] concurrently with the others and
/// under its own timeout: a check that exceeds it is reported as
/// [`HealthStatus::fail`] with a `timed out after ...` detail, so a readiness
/// endpoint answers 503 instead of hanging.
#[async_trait::async_trait]
pub trait IHealthCheck: Send + Sync {
    /// Probe name reported in the `checks` array of the health payload.
    fn name(&self) -> &str;

    /// Run the probe: `pass` = healthy, `warn` = degraded but serving,
    /// `fail` = not ready.
    async fn check(&self) -> HealthStatus;
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HealthStatus {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl HealthStatus {
    pub fn pass() -> Self {
        Self {
            status: "pass",
            detail: None,
        }
    }
    pub fn warn(d: impl Into<String>) -> Self {
        Self {
            status: "warn",
            detail: Some(d.into()),
        }
    }
    pub fn fail(d: impl Into<String>) -> Self {
        Self {
            status: "fail",
            detail: Some(d.into()),
        }
    }
}

/// A single health check entry in a registry snapshot.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HealthCheckEntry {
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone)]
enum HealthCheck {
    Sync(HealthCheckFn),
    Async {
        check: Arc<dyn IHealthCheck>,
        timeout: Duration,
    },
}

pub struct HealthCheckRegistry {
    checks: Mutex<Vec<(String, HealthCheck)>>,
}

fn entry(name: &str, status: HealthStatus) -> HealthCheckEntry {
    HealthCheckEntry {
        name: name.to_string(),
        status: status.status.to_string(),
        detail: status.detail,
    }
}

/// Sync view of a registered check.
///
/// An async check cannot be awaited here, so it is reported as `warn` instead of
/// running it on the caller's thread or pretending it passed. `warn` keeps
/// readiness endpoints at 200 and does not fail liveness; use
/// [`HealthCheckRegistry::snapshot_async`] for the real verdict.
fn sync_entry(name: &str, check: &HealthCheck) -> HealthCheckEntry {
    match check {
        HealthCheck::Sync(f) => entry(name, f()),
        HealthCheck::Async { .. } => HealthCheckEntry {
            name: name.to_string(),
            status: "warn".to_string(),
            detail: Some(format!(
                "async health check '{name}' requires async evaluation"
            )),
        },
    }
}

/// Overall status: `fail` dominates `warn`, which dominates `pass`; empty = pass.
fn overall_status_of(entries: &[HealthCheckEntry]) -> &'static str {
    if entries.iter().any(|entry| entry.status == "fail") {
        "fail"
    } else if entries.iter().any(|entry| entry.status == "warn") {
        "warn"
    } else {
        "pass"
    }
}

fn health_body(entries: &[HealthCheckEntry], overall: &'static str) -> Vec<u8> {
    let body = if entries.is_empty() {
        serde_json::json!({ "status": overall })
    } else {
        serde_json::json!({
            "status": overall,
            "checks": entries
        })
    };
    serde_json::to_vec(&body).unwrap_or_default()
}

impl HealthCheckRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            checks: Mutex::new(Vec::new()),
        })
    }

    /// Register a synchronous probe.
    ///
    /// The closure runs on the calling thread. Prefer
    /// [`Self::register_async`] for anything that blocks on I/O: a sync probe
    /// runs inside the request task and cannot be timed out.
    pub fn register(self: &Arc<Self>, name: impl Into<String>, check: HealthCheckFn) {
        if let Ok(mut checks) = self.checks.lock() {
            checks.push((name.into(), HealthCheck::Sync(check)));
        }
    }

    /// Register an async probe with its own timeout.
    ///
    /// The name comes from [`IHealthCheck::name`]. Timeouts are independent per
    /// check: one slow dependency becomes a single `fail` entry (503) while the
    /// other probes still report their real status.
    pub fn register_async(self: &Arc<Self>, check: Arc<dyn IHealthCheck>, timeout: Duration) {
        let name = check.name().to_string();
        if let Ok(mut checks) = self.checks.lock() {
            checks.push((name, HealthCheck::Async { check, timeout }));
        }
    }

    /// `true` when no check reports `fail` in the **synchronous** view.
    ///
    /// Async checks appear as `warn` here, so they never make this return
    /// `false`; use [`Self::all_healthy_async`] for the authoritative answer.
    pub fn all_healthy(&self) -> bool {
        let checks = match self.checks.lock() {
            Ok(c) => c,
            Err(_) => return false,
        };
        checks
            .iter()
            .all(|(name, check)| sync_entry(name, check).status != "fail")
    }

    /// 返回所有健康检查项的当前快照（同步视图）。
    ///
    /// Async 检查不会在这里执行，而是以 `warn` 计入，detail 说明需要异步求值；
    /// 需要真实结论时用 [`Self::snapshot_async`]。
    pub fn snapshot(&self) -> Vec<HealthCheckEntry> {
        let checks = match self.checks.lock() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        checks
            .iter()
            .map(|(name, check)| sync_entry(name, check))
            .collect()
    }

    /// 计算整体状态（同步视图）：fail 优先于 warn 优先于 pass；空列表视为 pass。
    /// Mutex poisoned 时保守返回 "fail"。
    pub fn overall_status(&self) -> &'static str {
        let checks = match self.checks.lock() {
            Ok(c) => c,
            Err(_) => return "fail",
        };
        let entries: Vec<HealthCheckEntry> = checks
            .iter()
            .map(|(name, check)| sync_entry(name, check))
            .collect();
        overall_status_of(&entries)
    }

    /// 并发执行所有检查并返回快照，顺序与注册顺序一致。
    ///
    /// Sync 检查照常执行，async 检查并发执行且各自受注册时的 timeout 约束：
    /// 超时记为 `fail`（detail 含 `timed out after ...`），整体状态随即变为
    /// fail，readiness 端点返回 503。
    pub async fn snapshot_async(&self) -> Vec<HealthCheckEntry> {
        let checks = match self.checks.lock() {
            Ok(c) => c.clone(),
            Err(_) => return Vec::new(),
        };
        join_all(checks.iter().map(|(name, check)| async move {
            match check {
                HealthCheck::Sync(f) => entry(name, f()),
                HealthCheck::Async { check, timeout } => {
                    let status = match tokio::time::timeout(*timeout, check.check()).await {
                        Ok(status) => status,
                        Err(_) => {
                            HealthStatus::fail(format!("timed out after {}ms", timeout.as_millis()))
                        }
                    };
                    entry(name, status)
                }
            }
        }))
        .await
    }

    /// 计算整体状态（异步视图）：并发执行所有检查，超时按 `fail` 处理。
    pub async fn overall_status_async(&self) -> &'static str {
        let entries = self.snapshot_async().await;
        overall_status_of(&entries)
    }

    /// `true` when no check reports `fail` in the **async** view (timeouts included).
    pub async fn all_healthy_async(&self) -> bool {
        self.snapshot_async()
            .await
            .iter()
            .all(|entry| entry.status != "fail")
    }
}

/// Build an RFC 8407 `application/health+json` body from the registry
/// (synchronous view; async checks count as `warn`).
///
/// When the registry has no checks, returns `{"status":"pass"}`.
/// Otherwise returns `{"status":<overall>,"checks":[...]}`.
pub fn build_health_response(registry: &HealthCheckRegistry) -> Vec<u8> {
    let entries = registry.snapshot();
    let overall = registry.overall_status();
    health_body(&entries, overall)
}

/// Async counterpart of [`build_health_response`]: evaluates async probes
/// concurrently and applies their timeouts.
pub async fn build_health_response_async(registry: &HealthCheckRegistry) -> Vec<u8> {
    let entries = registry.snapshot_async().await;
    let overall = overall_status_of(&entries);
    health_body(&entries, overall)
}

/// HTTP status for a health overall status: `fail` → 503, otherwise 200.
pub fn health_http_status(overall: &str) -> u16 {
    if overall == "fail" {
        503
    } else {
        200
    }
}

/// Dynamic health endpoint — evaluates registered probes on each request.
pub struct HealthCheckEndpoint {
    registry: Arc<HealthCheckRegistry>,
}

impl HealthCheckEndpoint {
    pub fn new(registry: Arc<HealthCheckRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl IEndpoint for HealthCheckEndpoint {
    async fn handle(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        // Evaluate once: snapshot_async is the only place probes run, so a
        // request executes each check exactly once even though it needs both
        // the entries and the overall status.
        let entries = self.registry.snapshot_async().await;
        let overall = overall_status_of(&entries);
        let body = health_body(&entries, overall);
        ctx.response_mut().set_status(health_http_status(overall));
        ctx.response_mut()
            .set_header("content-type", "application/health+json");
        ctx.response_mut().write_bytes(body).await
    }
}
