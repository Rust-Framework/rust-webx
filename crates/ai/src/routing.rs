//! Model routing: turn a public alias into an ordered attempt plan.
//!
//! The router is deliberately stateless apart from [`HealthRegistry`]: it reads
//! an immutable [`ModelCatalog`] snapshot, filters candidates by capabilities
//! and recent health, orders them by the alias strategy, and returns a
//! [`RoutePlan`]. Executing attempts (first-byte rules, retries, timeouts) is a
//! separate concern so routing policy stays unit-testable without I/O.

use crate::provider::{
    CatalogHandle, ChatProvider, ModelCatalog, ModelKey, ModelSpec, ProviderCapabilities,
    RouteStrategy, RouteTarget,
};
use async_trait::async_trait;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Failure/cooldown thresholds for [`HealthRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthPolicy {
    /// Consecutive failures that open the cooldown.
    pub failure_threshold: u32,
    /// How long a model stays out of the plan after opening.
    pub cooldown: Duration,
}

impl Default for HealthPolicy {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            cooldown: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Default)]
struct HealthEntry {
    consecutive_failures: u32,
    cooldown_until: Option<Instant>,
    latency_ewma_ms: f64,
    samples: u64,
}

/// One model's health at a point in time, for readiness/metrics.
#[derive(Debug, Clone)]
pub struct ModelHealth {
    /// Upstream key.
    pub model: ModelKey,
    /// Consecutive failed attempts since the last success.
    pub consecutive_failures: u32,
    /// Whether the cooldown window is still open.
    pub cooling_down: bool,
    /// Observed latency EWMA, once the model has succeeded at least once.
    pub latency_ewma_ms: Option<f64>,
}

/// Per-model health used by the router.
///
/// In-memory and process-local, like [`MemoryQuotaStore`](crate::quota::MemoryQuotaStore).
/// A multi-instance deployment can share outcomes through a store, but the
/// router only needs the local view.
pub struct HealthRegistry {
    policy: HealthPolicy,
    entries: Mutex<HashMap<ModelKey, HealthEntry>>,
}

impl HealthRegistry {
    /// Build a registry with `policy`.
    pub fn new(policy: HealthPolicy) -> Self {
        Self {
            policy,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// The configured policy.
    pub fn policy(&self) -> HealthPolicy {
        self.policy
    }

    /// Whether the model may be attempted at `now`.
    pub fn is_available(&self, key: &ModelKey, now: Instant) -> bool {
        self.lock()
            .get(key)
            .and_then(|entry| entry.cooldown_until)
            .map(|until| until <= now)
            .unwrap_or(true)
    }

    /// Record a successful attempt; resets failures and updates the EWMA.
    pub fn record_success(&self, key: &ModelKey, latency: Duration) {
        let mut entries = self.lock();
        let entry = entries.entry(key.clone()).or_default();
        let millis = latency.as_secs_f64() * 1000.0;
        entry.latency_ewma_ms = if entry.samples == 0 {
            millis
        } else {
            entry.latency_ewma_ms * 0.8 + millis * 0.2
        };
        entry.samples = entry.samples.saturating_add(1);
        entry.consecutive_failures = 0;
        entry.cooldown_until = None;
    }

    /// Record a failed attempt; opens the cooldown at the threshold.
    pub fn record_failure(&self, key: &ModelKey) {
        let mut entries = self.lock();
        let entry = entries.entry(key.clone()).or_default();
        entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
        if entry.consecutive_failures >= self.policy.failure_threshold.max(1) {
            entry.cooldown_until = Some(Instant::now() + self.policy.cooldown);
        }
    }

    /// Observed latency, if the model has succeeded at least once.
    pub fn latency_ewma_ms(&self, key: &ModelKey) -> Option<f64> {
        self.lock()
            .get(key)
            .filter(|entry| entry.samples > 0)
            .map(|entry| entry.latency_ewma_ms)
    }

    /// Current consecutive failure count.
    pub fn consecutive_failures(&self, key: &ModelKey) -> u32 {
        self.lock()
            .get(key)
            .map(|entry| entry.consecutive_failures)
            .unwrap_or(0)
    }

    /// Snapshot every observed model at `now`.
    ///
    /// Readiness checks use this to decide whether the gateway has any usable
    /// upstream left; the hot path never enumerates this map.
    pub fn snapshot_at(&self, now: Instant) -> Vec<ModelHealth> {
        self.lock()
            .iter()
            .map(|(model, entry)| ModelHealth {
                model: model.clone(),
                consecutive_failures: entry.consecutive_failures,
                cooling_down: entry.cooldown_until.is_some_and(|until| until > now),
                latency_ewma_ms: (entry.samples > 0).then_some(entry.latency_ewma_ms),
            })
            .collect()
    }

    /// Snapshot every observed model at the current instant.
    pub fn snapshot(&self) -> Vec<ModelHealth> {
        self.snapshot_at(Instant::now())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ModelKey, HealthEntry>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for HealthRegistry {
    fn default() -> Self {
        Self::new(HealthPolicy::default())
    }
}

/// What the caller needs from a model.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteRequest {
    /// Public model/alias.
    pub model: String,
    /// Capabilities that must be present.
    pub required: ProviderCapabilities,
    /// Absolute deadline for the whole request, if known.
    pub deadline: Option<Instant>,
}

impl RouteRequest {
    /// A request for `model` with no capability requirement.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            required: ProviderCapabilities::none(),
            deadline: None,
        }
    }

    /// Require capabilities.
    pub fn require(mut self, required: ProviderCapabilities) -> Self {
        self.required = required;
        self
    }

    /// Set the request deadline.
    pub fn deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }
}

/// One ordered attempt in a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    /// Upstream model.
    pub model: ModelKey,
    /// Suggested per-attempt timeout.
    pub timeout: Duration,
}

/// The router's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutePlan {
    /// Public alias the plan was built for.
    pub alias: String,
    /// Strategy used to order attempts.
    pub strategy: RouteStrategy,
    /// Ordered candidates; the executor tries them in order.
    pub attempts: Vec<Attempt>,
    /// Absolute deadline for the whole request.
    pub deadline: Instant,
}

/// Why no plan could be built.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RouteError {
    /// The public alias is not registered.
    #[error("unknown model alias: {0}")]
    UnknownModel(String),
    /// Every candidate was disabled, unhealthy, missing, or lacked a capability.
    #[error("no candidate for model alias: {0}")]
    NoCandidate(String),
    /// The request/plan parameters are invalid.
    #[error("invalid route request: {0}")]
    Invalid(String),
}

impl RouteError {
    /// Suggested HTTP status: unknown alias `404`, no candidate `503`.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::UnknownModel(_) => 404,
            Self::NoCandidate(_) => 503,
            Self::Invalid(_) => 400,
        }
    }
}

/// Route planning port.
#[async_trait]
pub trait IRoutePlanner: Send + Sync {
    /// Build an ordered plan for `request`.
    async fn plan(&self, request: &RouteRequest) -> Result<RoutePlan, RouteError>;
}

/// Catalog-backed router.
pub struct CatalogRouter {
    catalog: CatalogHandle,
    health: Arc<HealthRegistry>,
    default_timeout: Duration,
    max_attempts: usize,
}

impl CatalogRouter {
    /// A router with 120s attempts, 3 max attempts and default health policy.
    pub fn new(catalog: Arc<ModelCatalog>) -> Self {
        Self::from_handle(CatalogHandle::new(catalog))
    }

    /// A router sharing a hot-reloadable catalog handle.
    pub fn from_handle(catalog: CatalogHandle) -> Self {
        Self {
            catalog,
            health: Arc::new(HealthRegistry::default()),
            default_timeout: Duration::from_secs(120),
            max_attempts: 3,
        }
    }

    /// The shared catalog handle.
    pub fn handle(&self) -> CatalogHandle {
        self.catalog.clone()
    }

    /// Replace the catalog snapshot (stale versions are ignored).
    pub fn reload(&self, catalog: Arc<ModelCatalog>) -> bool {
        self.catalog.reload(catalog)
    }

    /// Use a shared health registry.
    pub fn with_health(mut self, health: Arc<HealthRegistry>) -> Self {
        self.health = health;
        self
    }

    /// Set the per-attempt timeout suggestion.
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout.max(Duration::from_millis(1));
        self
    }

    /// Set the maximum number of attempts.
    pub fn with_max_attempts(mut self, max_attempts: usize) -> Self {
        self.max_attempts = max_attempts.max(1);
        self
    }

    /// The current catalog snapshot.
    pub fn catalog(&self) -> Arc<ModelCatalog> {
        self.catalog.current()
    }

    /// The shared health registry.
    pub fn health(&self) -> &Arc<HealthRegistry> {
        &self.health
    }

    fn candidate_is_usable<'a>(
        &self,
        catalog: &'a ModelCatalog,
        target: &RouteTarget,
        required: ProviderCapabilities,
        now: Instant,
    ) -> Option<&'a ModelSpec> {
        let spec = catalog.models.get(&target.model)?;
        if !spec.enabled {
            return None;
        }
        if !spec.capabilities.supports(required) {
            return None;
        }
        let provider: Option<&Arc<dyn ChatProvider>> =
            catalog.providers.get(&target.model.provider);
        provider?;
        if !self.health.is_available(&target.model, now) {
            return None;
        }
        Some(spec)
    }
}

#[async_trait]
impl IRoutePlanner for CatalogRouter {
    async fn plan(&self, request: &RouteRequest) -> Result<RoutePlan, RouteError> {
        let catalog = self.catalog.current();
        let alias = catalog
            .aliases
            .get(&request.model)
            .ok_or_else(|| RouteError::UnknownModel(request.model.clone()))?;

        let now = Instant::now();
        let mut candidates: Vec<RouteTarget> = alias
            .targets
            .iter()
            .filter(|target| {
                self.candidate_is_usable(&catalog, target, request.required, now)
                    .is_some()
            })
            .cloned()
            .collect();

        match alias.strategy {
            RouteStrategy::Priority | RouteStrategy::Fallback | RouteStrategy::Capability => {}
            RouteStrategy::Weighted => {
                candidates.sort_by(|left, right| {
                    right
                        .weight
                        .cmp(&left.weight)
                        .then_with(|| left.model.cmp(&right.model))
                });
            }
            RouteStrategy::LeastLatency => {
                candidates.sort_by(|left, right| {
                    let left_latency = self
                        .health
                        .latency_ewma_ms(&left.model)
                        .unwrap_or(f64::INFINITY);
                    let right_latency = self
                        .health
                        .latency_ewma_ms(&right.model)
                        .unwrap_or(f64::INFINITY);
                    left_latency
                        .partial_cmp(&right_latency)
                        .unwrap_or(Ordering::Equal)
                        .then_with(|| right.weight.cmp(&left.weight))
                });
            }
        }

        if candidates.is_empty() {
            return Err(RouteError::NoCandidate(request.model.clone()));
        }

        candidates.truncate(self.max_attempts);
        let deadline = request
            .deadline
            .unwrap_or_else(|| now + self.default_timeout);
        let attempts = candidates
            .into_iter()
            .map(|target| Attempt {
                model: target.model,
                timeout: self.default_timeout,
            })
            .collect();

        Ok(RoutePlan {
            alias: request.model.clone(),
            strategy: alias.strategy,
            attempts,
            deadline,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{CanonicalRequest, CanonicalResponse};
    use crate::provider::ProviderError;

    struct MockProvider {
        id: String,
        capabilities: ProviderCapabilities,
    }

    impl MockProvider {
        fn arc(id: &str, capabilities: ProviderCapabilities) -> Arc<dyn ChatProvider> {
            Arc::new(Self {
                id: id.into(),
                capabilities,
            })
        }
    }

    #[async_trait]
    impl ChatProvider for MockProvider {
        fn id(&self) -> &str {
            &self.id
        }

        fn capabilities(&self) -> ProviderCapabilities {
            self.capabilities
        }

        async fn complete(
            &self,
            _request: CanonicalRequest,
        ) -> Result<CanonicalResponse, ProviderError> {
            Err(ProviderError::Unsupported("mock".into()))
        }
    }

    fn catalog() -> Arc<ModelCatalog> {
        let streaming = ProviderCapabilities::chat_stream();
        let mut catalog = ModelCatalog::new(1);
        catalog.add_provider(MockProvider::arc("a", streaming));
        catalog.add_provider(MockProvider::arc("b", streaming.tools()));
        catalog.add_provider(MockProvider::arc("c", streaming.tools().vision()));

        catalog
            .add_model(ModelSpec::new(ModelKey::new("a", "model-a")).with_capabilities(streaming));
        catalog.add_model(
            ModelSpec::new(ModelKey::new("b", "model-b")).with_capabilities(streaming.tools()),
        );
        catalog.add_model(
            ModelSpec::new(ModelKey::new("c", "model-c"))
                .with_capabilities(streaming.tools().vision()),
        );

        catalog.add_alias(
            "fallback",
            crate::provider::Alias::new(
                RouteStrategy::Fallback,
                vec![
                    RouteTarget::new(ModelKey::new("a", "model-a")),
                    RouteTarget::new(ModelKey::new("b", "model-b")),
                    RouteTarget::new(ModelKey::new("c", "model-c")),
                ],
            ),
        );
        catalog.add_alias(
            "tools",
            crate::provider::Alias::new(
                RouteStrategy::Capability,
                vec![
                    RouteTarget::new(ModelKey::new("a", "model-a")),
                    RouteTarget::new(ModelKey::new("b", "model-b")),
                ],
            ),
        );
        catalog.add_alias(
            "vision",
            crate::provider::Alias::new(
                RouteStrategy::Capability,
                vec![
                    RouteTarget::new(ModelKey::new("a", "model-a")),
                    RouteTarget::new(ModelKey::new("c", "model-c")),
                ],
            ),
        );
        catalog.add_alias(
            "weighted",
            crate::provider::Alias::new(
                RouteStrategy::Weighted,
                vec![
                    RouteTarget::new(ModelKey::new("a", "model-a")).with_weight(1),
                    RouteTarget::new(ModelKey::new("b", "model-b")).with_weight(5),
                ],
            ),
        );
        catalog.add_alias(
            "latency",
            crate::provider::Alias::new(
                RouteStrategy::LeastLatency,
                vec![
                    RouteTarget::new(ModelKey::new("a", "model-a")),
                    RouteTarget::new(ModelKey::new("b", "model-b")),
                ],
            ),
        );
        Arc::new(catalog)
    }

    fn model_names(plan: &RoutePlan) -> Vec<String> {
        plan.attempts
            .iter()
            .map(|attempt| attempt.model.model.clone())
            .collect()
    }

    #[tokio::test]
    async fn fallback_keeps_catalog_order() {
        let router = CatalogRouter::new(catalog());
        let plan = router
            .plan(&RouteRequest::new("fallback"))
            .await
            .expect("plan");
        assert_eq!(model_names(&plan), ["model-a", "model-b", "model-c"]);
        assert_eq!(plan.strategy, RouteStrategy::Fallback);
    }

    #[tokio::test]
    async fn capability_filter_removes_unsupported_models() {
        let router = CatalogRouter::new(catalog());
        let plan = router
            .plan(&RouteRequest::new("tools").require(ProviderCapabilities::none().tools()))
            .await
            .expect("plan");
        assert_eq!(model_names(&plan), ["model-b"]);

        let plan = router
            .plan(&RouteRequest::new("vision").require(ProviderCapabilities::none().vision()))
            .await
            .expect("vision plan");
        assert_eq!(model_names(&plan), ["model-c"]);
    }

    #[tokio::test]
    async fn weighted_orders_higher_weight_first() {
        let router = CatalogRouter::new(catalog());
        let plan = router
            .plan(&RouteRequest::new("weighted"))
            .await
            .expect("plan");
        assert_eq!(model_names(&plan), ["model-b", "model-a"]);
    }

    #[tokio::test]
    async fn least_latency_uses_the_ewma() {
        let catalog = catalog();
        let health = Arc::new(HealthRegistry::default());
        let router = CatalogRouter::new(catalog).with_health(Arc::clone(&health));

        health.record_success(&ModelKey::new("a", "model-a"), Duration::from_millis(100));
        health.record_success(&ModelKey::new("b", "model-b"), Duration::from_millis(20));

        let plan = router
            .plan(&RouteRequest::new("latency"))
            .await
            .expect("plan");
        assert_eq!(model_names(&plan), ["model-b", "model-a"]);
    }

    #[tokio::test]
    async fn unhealthy_models_are_excluded_until_cooldown_ends() {
        let catalog = catalog();
        let health = Arc::new(HealthRegistry::new(HealthPolicy {
            failure_threshold: 1,
            cooldown: Duration::from_millis(1),
        }));
        let router = CatalogRouter::new(catalog).with_health(Arc::clone(&health));

        health.record_failure(&ModelKey::new("a", "model-a"));
        let plan = router
            .plan(&RouteRequest::new("fallback"))
            .await
            .expect("plan");
        assert_eq!(model_names(&plan), ["model-b", "model-c"]);

        tokio::time::sleep(Duration::from_millis(10)).await;
        let plan = router
            .plan(&RouteRequest::new("fallback"))
            .await
            .expect("plan after cooldown");
        assert_eq!(
            model_names(&plan),
            ["model-a", "model-b", "model-c"],
            "the model returns after its cooldown"
        );
    }

    #[tokio::test]
    async fn unknown_alias_is_an_error() {
        let router = CatalogRouter::new(catalog());
        let error = router
            .plan(&RouteRequest::new("missing"))
            .await
            .expect_err("unknown alias");
        assert_eq!(error.http_status(), 404);
        assert!(matches!(error, RouteError::UnknownModel(_)));
    }

    #[tokio::test]
    async fn no_candidate_is_a_503_class_error() {
        let router = CatalogRouter::new(catalog());
        let error = router
            .plan(&RouteRequest::new("tools").require(ProviderCapabilities::none().audio()))
            .await
            .expect_err("no model supports audio");
        assert_eq!(error.http_status(), 503);
        assert!(matches!(error, RouteError::NoCandidate(_)));
    }

    #[tokio::test]
    async fn max_attempts_truncates_the_plan() {
        let router = CatalogRouter::new(catalog()).with_max_attempts(1);
        let plan = router
            .plan(&RouteRequest::new("fallback"))
            .await
            .expect("plan");
        assert_eq!(model_names(&plan), ["model-a"]);
    }

    #[test]
    fn health_ewma_moves_toward_new_samples() {
        let health = HealthRegistry::default();
        let key = ModelKey::new("a", "model-a");
        health.record_success(&key, Duration::from_millis(100));
        health.record_success(&key, Duration::from_millis(20));
        let ewma = health.latency_ewma_ms(&key).expect("samples");
        assert!((ewma - 84.0).abs() < 0.001);
        assert_eq!(health.consecutive_failures(&key), 0);
    }

    #[test]
    fn health_failure_threshold_opens_cooldown() {
        let health = HealthRegistry::new(HealthPolicy {
            failure_threshold: 2,
            cooldown: Duration::from_secs(5),
        });
        let key = ModelKey::new("a", "model-a");
        let now = Instant::now();

        health.record_failure(&key);
        assert!(health.is_available(&key, now));
        health.record_failure(&key);
        assert!(!health.is_available(&key, now));
        assert_eq!(health.consecutive_failures(&key), 2);
    }
}
