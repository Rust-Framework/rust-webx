//! Gateway readiness: are any upstream models usable?
//!
//! A gateway can be alive (liveness) while every upstream is cooling down or
//! its circuit is open. [`GatewayHealth`] turns the executor's
//! [`HealthRegistry`] and optional [`CircuitBreaker`] into a `/health/ready`
//! check:
//!
//! * `pass` — no observed problem, or at least one known model is usable;
//! * `warn` — some models are cooling down / circuit-open, but others remain;
//! * `fail` — every known model is unavailable (or every catalog model, when a
//!   catalog handle is supplied).
//!
//! Registration (the closure is sync, like every host health check):
//!
//! ```ignore
//! let health = Arc::new(
//!     GatewayHealth::new(router.health().clone())
//!         .with_catalog(handle.clone())
//!         .with_circuit_breaker(circuit.clone()),
//! );
//! Host::builder()
//!     .register(move |svc| svc.instance(Arc::clone(&health)))
//!     .add_health_check("gateway-upstreams", health.check())
//!     .build();
//! ```
//!
//! The detail only contains counts and model keys (`provider/model`), never
//! tenant/key identifiers or prompts.

use std::collections::BTreeSet;
use std::sync::Arc;

use webx::{HealthCheckFn, HealthStatus};
use webx_ai::{CatalogHandle, CircuitBreaker, CircuitState, HealthRegistry};

/// Readiness view over the executor's health/circuit state.
pub struct GatewayHealth {
    health: Arc<HealthRegistry>,
    circuit: Option<Arc<CircuitBreaker>>,
    catalog: Option<CatalogHandle>,
}

impl GatewayHealth {
    /// Build a check over the router's health registry.
    pub fn new(health: Arc<HealthRegistry>) -> Self {
        Self {
            health,
            circuit: None,
            catalog: None,
        }
    }

    /// Include the circuit breaker's open models.
    pub fn with_circuit_breaker(mut self, circuit: Arc<CircuitBreaker>) -> Self {
        self.circuit = Some(circuit);
        self
    }

    /// Include the catalog so "every known model" means every catalog model,
    /// not only the ones that received traffic.
    pub fn with_catalog(mut self, catalog: CatalogHandle) -> Self {
        self.catalog = Some(catalog);
        self
    }

    /// Evaluate the current readiness.
    pub fn status(&self) -> HealthStatus {
        let mut known = BTreeSet::new();
        let mut unavailable = BTreeSet::new();

        for entry in self.health.snapshot() {
            let key = format!("{}/{}", entry.model.provider, entry.model.model);
            known.insert(key.clone());
            if entry.cooling_down {
                unavailable.insert(key);
            }
        }
        if let Some(circuit) = &self.circuit {
            for entry in circuit.snapshot() {
                let key = format!("{}/{}", entry.model.provider, entry.model.model);
                known.insert(key.clone());
                if entry.state == CircuitState::Open {
                    unavailable.insert(key);
                }
            }
        }
        if let Some(catalog) = &self.catalog {
            for key in catalog.current().models.keys() {
                known.insert(format!("{}/{}", key.provider, key.model));
            }
        }

        if known.is_empty() {
            return HealthStatus::pass();
        }
        if unavailable.len() >= known.len() {
            return HealthStatus::fail(format!(
                "all {} known upstream model(s) are cooling down or circuit-open",
                known.len()
            ));
        }
        if !unavailable.is_empty() {
            return HealthStatus::warn(format!(
                "{} of {} upstream model(s) are cooling down or circuit-open",
                unavailable.len(),
                known.len()
            ));
        }
        HealthStatus::pass()
    }

    /// A host health-check closure for `HostBuilder::add_health_check`.
    pub fn check(self: &Arc<Self>) -> HealthCheckFn {
        let this = Arc::clone(self);
        Arc::new(move || this.status())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use webx_ai::{
        Alias, CatalogHandle, HealthPolicy, ModelCatalog, ModelKey, ModelSpec, RouteStrategy,
        RouteTarget,
    };

    fn key(model: &str) -> ModelKey {
        ModelKey::new("p", model)
    }

    fn catalog(handle_models: &[&str]) -> CatalogHandle {
        let mut catalog = ModelCatalog::new(1);
        for model in handle_models {
            catalog.add_model(ModelSpec::new(key(model)));
        }
        let target = RouteTarget::new(key(handle_models[0]));
        catalog.add_alias("fast", Alias::new(RouteStrategy::Fallback, vec![target]));
        CatalogHandle::new(Arc::new(catalog))
    }

    #[test]
    fn no_observations_is_ready() {
        let health = Arc::new(GatewayHealth::new(Arc::new(HealthRegistry::default())));
        assert_eq!(health.status().status, "pass");
    }

    #[test]
    fn every_catalog_model_cooling_down_fails_readiness() {
        let registry = Arc::new(HealthRegistry::new(HealthPolicy {
            failure_threshold: 1,
            cooldown: Duration::from_secs(60),
        }));
        registry.record_failure(&key("a"));
        registry.record_failure(&key("b"));
        let health = Arc::new(GatewayHealth::new(registry).with_catalog(catalog(&["a", "b"])));

        let status = health.status();
        assert_eq!(status.status, "fail");
        assert!(status.detail.unwrap_or_default().contains("2"));
    }

    #[test]
    fn one_healthy_model_keeps_readiness_at_warn() {
        let registry = Arc::new(HealthRegistry::new(HealthPolicy {
            failure_threshold: 1,
            cooldown: Duration::from_secs(60),
        }));
        registry.record_failure(&key("a"));
        let health = Arc::new(GatewayHealth::new(registry).with_catalog(catalog(&["a", "b"])));

        let status = health.status();
        assert_eq!(status.status, "warn");
        assert!(status.detail.unwrap_or_default().contains("1 of 2"));
    }

    #[test]
    fn an_open_circuit_counts_as_unavailable() {
        let registry = Arc::new(HealthRegistry::default());
        let circuit = Arc::new(CircuitBreaker::new(1, Duration::from_secs(60)));
        circuit.record_failure(&key("a"));
        let health = Arc::new(
            GatewayHealth::new(registry)
                .with_catalog(catalog(&["a", "b"]))
                .with_circuit_breaker(circuit),
        );

        assert_eq!(health.status().status, "warn");
    }

    #[test]
    fn an_expired_circuit_cooldown_is_ready_for_a_probe() {
        let registry = Arc::new(HealthRegistry::default());
        let circuit = Arc::new(CircuitBreaker::new(1, Duration::from_millis(1)));
        circuit.record_failure(&key("a"));
        std::thread::sleep(Duration::from_millis(5));
        let health = Arc::new(
            GatewayHealth::new(registry)
                .with_catalog(catalog(&["a"]))
                .with_circuit_breaker(circuit),
        );

        assert_eq!(health.status().status, "pass");
    }

    #[test]
    fn snapshot_reports_cooling_and_latency() {
        let registry = HealthRegistry::new(HealthPolicy {
            failure_threshold: 3,
            cooldown: Duration::from_secs(1),
        });
        registry.record_success(&key("a"), Duration::from_millis(20));
        registry.record_failure(&key("a"));
        let snapshot = registry.snapshot_at(Instant::now());
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].consecutive_failures, 1);
        assert!(!snapshot[0].cooling_down);
        assert!(snapshot[0].latency_ewma_ms.is_some());
    }
}
