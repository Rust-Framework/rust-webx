//! Enhanced health check endpoints for Kubernetes.
//!
//! Provides HealthCheckRegistry and typed HealthStatus.

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
}
