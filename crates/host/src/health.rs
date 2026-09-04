//! Enhanced health check endpoints for Kubernetes.
//!
//! Provides HealthCheckRegistry and typed HealthStatus.
//! Endpoints follow RFC 8407 (`application/health+json` content type).

use rust_webx_core::error::Result;
use rust_webx_core::http::IHttpContext;
use rust_webx_core::routing::IEndpoint;
use std::sync::{Arc, Mutex};

pub type HealthCheckFn = Arc<dyn Fn() -> HealthStatus + Send + Sync>;

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

pub struct HealthCheckRegistry {
    checks: Mutex<Vec<(String, HealthCheckFn)>>,
}

impl HealthCheckRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            checks: Mutex::new(Vec::new()),
        })
    }
    pub fn register(self: &Arc<Self>, name: impl Into<String>, check: HealthCheckFn) {
        if let Ok(mut checks) = self.checks.lock() {
            checks.push((name.into(), check));
        }
    }
    pub fn all_healthy(&self) -> bool {
        let checks = match self.checks.lock() {
            Ok(c) => c.clone(),
            Err(_) => return false,
        };
        checks.iter().all(|(_, f)| f().status != "fail")
    }

    /// 返回所有健康检查项的当前快照。
    pub fn snapshot(&self) -> Vec<HealthCheckEntry> {
        let checks = match self.checks.lock() {
            Ok(c) => c.clone(),
            Err(_) => return Vec::new(),
        };
        checks
            .iter()
            .map(|(name, f)| {
                let s = f();
                HealthCheckEntry {
                    name: name.clone(),
                    status: s.status.to_string(),
                    detail: s.detail.clone(),
                }
            })
            .collect()
    }

    /// 计算整体状态：fail 优先于 warn 优先于 pass；空列表视为 pass。
    /// Mutex poisoned 时保守返回 "fail"。
    pub fn overall_status(&self) -> &'static str {
        let checks = match self.checks.lock() {
            Ok(c) => c.clone(),
            Err(_) => return "fail",
        };
        if checks.is_empty() {
            "pass"
        } else {
            let any_fail = checks.iter().any(|(_, f)| f().status == "fail");
            let any_warn = checks.iter().any(|(_, f)| f().status == "warn");
            if any_fail {
                "fail"
            } else if any_warn {
                "warn"
            } else {
                "pass"
            }
        }
    }
}

/// Build an RFC 8407 `application/health+json` body from the registry.
///
/// When the registry has no checks, returns `{"status":"pass"}`.
/// Otherwise returns `{"status":<overall>,"checks":[...]}`.
pub fn build_health_response(registry: &HealthCheckRegistry) -> Vec<u8> {
    let entries = registry.snapshot();
    let overall = registry.overall_status();
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
        let overall = self.registry.overall_status();
        let body = build_health_response(&self.registry);
        ctx.response_mut().set_status(health_http_status(overall));
        ctx.response_mut()
            .set_header("content-type", "application/health+json");
        ctx.response_mut().write_bytes(body).await
    }
}
