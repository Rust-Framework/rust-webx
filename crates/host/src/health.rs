//! Enhanced health check endpoints for Kubernetes.
//!
//! Provides HealthCheckRegistry and typed HealthStatus.
//! Endpoints follow RFC 8407 (`application/health+json` content type).

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
        self.checks
            .lock()
            .map(|c| c.iter().all(|(_, f)| f().status != "fail"))
            .unwrap_or(false)
    }

    /// 返回所有健康检查项的当前快照。
    pub fn snapshot(&self) -> Vec<HealthCheckEntry> {
        self.checks
            .lock()
            .map(|c| {
                c.iter()
                    .map(|(name, f)| {
                        let s = f();
                        HealthCheckEntry {
                            name: name.clone(),
                            status: s.status.to_string(),
                            detail: s.detail.clone(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 计算整体状态：fail 优先于 warn 优先于 pass；空列表视为 pass。
    /// Mutex poisoned 时保守返回 "fail"。
    pub fn overall_status(&self) -> &'static str {
        self.checks
            .lock()
            .map(|c| {
                let entries: Vec<_> = c.iter().map(|(_, f)| f()).collect();
                if entries.is_empty() {
                    "pass"
                } else if entries.iter().any(|e| e.status == "fail") {
                    "fail"
                } else if entries.iter().any(|e| e.status == "warn") {
                    "warn"
                } else {
                    "pass"
                }
            })
            .unwrap_or("fail")
    }
}
