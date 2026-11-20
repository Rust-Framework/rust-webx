//! Request executor: route ->reserve ->provider attempts ->settle/usage.
//!
//! The executor is the glue between the pure routing/quota/usage pieces and a
//! concrete [`ChatProvider`](crate::provider::ChatProvider). It enforces the two rules that cannot live in the
//! router:
//!
//! * **Reserve before calling upstream**, settle/release exactly once.
//! * **No retry after the first streamed byte**: non-stream calls may try the
//!   next candidate on a retryable error; a returned stream is final.
//!
//! Streaming settlement runs in a background task because the stream may end by
//! client disconnect (`Drop`), where awaiting is impossible.

use crate::cache::{request_cache_key, IGatewayCache};
use crate::cost::{ICostCalculator, NoCost};
use crate::ir::{CanonicalRequest, CanonicalResponse, ContentPart, StreamEvent};
use crate::provider::{
    CanonicalStream, CatalogHandle, ModelCatalog, ModelKey, ProviderCapabilities, ProviderError,
};
use crate::quota::{IQuotaStore, QuotaKey, Reservation, ReserveRequest};
use crate::resilience::{Bulkhead, BulkheadPermit, CircuitBreaker, CircuitState, RetryPolicy};
use crate::routing::{HealthRegistry, IRoutePlanner, RouteRequest};
use crate::usage::{ITokenizer, IUsageSink, Usage, UsageEvent, UsageStatus};
use futures_util::stream::Stream;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::OnceCell;
use webx_core::metrics::{Gauge, IMeter};

/// Identity/accounting metadata for one gateway request.
#[derive(Debug, Clone)]
pub struct GatewayContext {
    /// Gateway request id, also the usage idempotency key.
    pub request_id: String,
    /// Tenant that owns the API key.
    pub tenant_id: String,
    /// API key id (never the key itself).
    pub key_id: String,
    /// Public alias the client asked for.
    pub model_alias: String,
}

impl GatewayContext {
    /// Build a request context.
    pub fn new(
        request_id: impl Into<String>,
        tenant_id: impl Into<String>,
        key_id: impl Into<String>,
        model_alias: impl Into<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            tenant_id: tenant_id.into(),
            key_id: key_id.into(),
            model_alias: model_alias.into(),
        }
    }
}

/// A data-plane failure.
#[derive(Debug, Clone, Error)]
pub enum GatewayError {
    /// Routing could not produce a plan.
    #[error("routing failed: {0}")]
    Route(#[from] crate::routing::RouteError),
    /// Quota reservation failed.
    #[error("quota rejected: {0}")]
    Quota(#[from] crate::quota::QuotaError),
    /// The upstream provider failed.
    #[error("provider failed: {0}")]
    Provider(#[from] ProviderError),
    /// The catalog references a provider that is not registered.
    #[error("provider not registered: {0}")]
    NoProvider(String),
    /// A gateway invariant was violated.
    #[error("gateway internal error: {0}")]
    Internal(String),
}

impl GatewayError {
    /// Suggested HTTP status for the gateway surface.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Route(error) => error.http_status(),
            Self::Quota(error) => error.http_status(),
            Self::Provider(error) => error.http_status(),
            Self::NoProvider(_) => 503,
            Self::Internal(_) => 500,
        }
    }

    /// Whether the caller may retry the whole request.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Provider(error) => error.is_retryable(),
            Self::Quota(error) => error.http_status() == 503,
            _ => false,
        }
    }

    /// Provider retry hint, when present.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Provider(error) => error.retry_after(),
            Self::Quota(error) => error.retry_after(),
            _ => None,
        }
    }
}

/// A dry-run estimate: no provider call, no quota reservation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GatewayEstimate {
    /// Public alias.
    pub model: String,
    /// Estimated prompt tokens.
    pub prompt_tokens: u64,
    /// Completion bound used for the estimate.
    pub max_completion_tokens: u64,
    /// Estimated cost in micro-USD.
    pub estimated_cost_micros: u64,
}

/// Releases a quota reservation if the request future is dropped before it is
/// settled or explicitly released (client disconnect, host request timeout,
/// handler cancellation).
struct ReservationGuard {
    store: Option<Arc<dyn IQuotaStore>>,
    reservation: Option<Reservation>,
}

impl ReservationGuard {
    fn new(store: &Option<Arc<dyn IQuotaStore>>, reservation: Option<Reservation>) -> Self {
        Self {
            store: store.clone(),
            reservation,
        }
    }

    fn take(&mut self) -> Option<Reservation> {
        self.reservation.take()
    }
}

impl Drop for ReservationGuard {
    fn drop(&mut self) {
        let (Some(store), Some(reservation)) = (self.store.take(), self.reservation.take()) else {
            return;
        };
        // Drop cannot await. Release on the runtime; the lease TTL is the
        // fallback if the runtime is already gone.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(error) = store.release(reservation).await {
                    tracing::warn!(error = %error, "cancelled request lease release failed");
                }
            });
        }
    }
}

/// Data-plane entry point.
pub struct GatewayExecutor {
    catalog: Arc<ModelCatalog>,
    catalog_handle: Option<CatalogHandle>,
    router: Arc<dyn IRoutePlanner>,
    quota: Option<Arc<dyn IQuotaStore>>,
    usage: Option<Arc<dyn IUsageSink>>,
    tokenizer: Arc<dyn ITokenizer>,
    health: Option<Arc<HealthRegistry>>,
    retry: RetryPolicy,
    circuit: Option<Arc<CircuitBreaker>>,
    bulkhead: Option<Arc<Bulkhead>>,
    meter: Option<Arc<dyn IMeter>>,
    cache: Option<Arc<dyn IGatewayCache>>,
    cache_ttl: Duration,
    single_flight: Option<Arc<SingleFlight>>,
    cost: Arc<dyn ICostCalculator>,
    default_max_tokens: u32,
}

type FlightResult = Result<CanonicalResponse, GatewayError>;

struct SingleFlight {
    flights: Mutex<HashMap<String, Arc<OnceCell<FlightResult>>>>,
}

impl SingleFlight {
    fn new() -> Self {
        Self {
            flights: Mutex::new(HashMap::new()),
        }
    }

    fn cell(&self, key: &str) -> Arc<OnceCell<FlightResult>> {
        let mut flights = self
            .flights
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        flights
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone()
    }

    fn finish(&self, key: &str, cell: &Arc<OnceCell<FlightResult>>) {
        let mut flights = self
            .flights
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(current) = flights.get(key) {
            if Arc::ptr_eq(current, cell) {
                flights.remove(key);
            }
        }
    }
}

impl GatewayExecutor {
    /// Build an executor over a catalog and planner.
    pub fn new(catalog: Arc<ModelCatalog>, router: Arc<dyn IRoutePlanner>) -> Self {
        Self {
            catalog,
            catalog_handle: None,
            router,
            quota: None,
            usage: None,
            tokenizer: Arc::new(crate::usage::HeuristicTokenizer::default()),
            health: None,
            retry: RetryPolicy::default(),
            circuit: None,
            bulkhead: None,
            meter: None,
            cache: None,
            cache_ttl: Duration::from_secs(60),
            single_flight: None,
            cost: Arc::new(NoCost),
            default_max_tokens: 1024,
        }
    }

    /// Build an executor sharing a hot-reloadable catalog with the router.
    pub fn new_with_handle(handle: CatalogHandle, router: Arc<dyn IRoutePlanner>) -> Self {
        Self {
            catalog: handle.current(),
            catalog_handle: Some(handle),
            router,
            quota: None,
            usage: None,
            tokenizer: Arc::new(crate::usage::HeuristicTokenizer::default()),
            health: None,
            retry: RetryPolicy::default(),
            circuit: None,
            bulkhead: None,
            meter: None,
            cache: None,
            cache_ttl: Duration::from_secs(60),
            single_flight: None,
            cost: Arc::new(NoCost),
            default_max_tokens: 1024,
        }
    }

    /// Share a hot-reloadable catalog with the router.
    pub fn with_catalog_handle(mut self, handle: CatalogHandle) -> Self {
        self.catalog = handle.current();
        self.catalog_handle = Some(handle);
        self
    }

    fn current_catalog(&self) -> Arc<ModelCatalog> {
        self.catalog_handle
            .as_ref()
            .map(CatalogHandle::current)
            .unwrap_or_else(|| Arc::clone(&self.catalog))
    }

    /// The catalog snapshot currently used by this executor.
    pub fn catalog_snapshot(&self) -> Arc<ModelCatalog> {
        self.current_catalog()
    }

    /// Enable exact response caching for deterministic non-stream requests.
    pub fn with_cache(mut self, cache: Arc<dyn IGatewayCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Estimate prompt/completion tokens and cost without calling a provider or
    /// reserving quota.
    pub fn estimate(&self, ctx: &GatewayContext, request: &CanonicalRequest) -> GatewayEstimate {
        let prompt_tokens = estimate_prompt_tokens(&*self.tokenizer, &ctx.model_alias, request);
        let max_completion_tokens = u64::from(
            request
                .sampling
                .max_tokens
                .unwrap_or(self.default_max_tokens),
        );
        let usage = Usage {
            prompt_tokens,
            completion_tokens: max_completion_tokens,
            ..Usage::ZERO
        };
        GatewayEstimate {
            model: ctx.model_alias.clone(),
            prompt_tokens,
            max_completion_tokens,
            estimated_cost_micros: self.cost.cost_micros(&ctx.model_alias, &usage),
        }
    }

    /// Cache TTL (default 60s).
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// Share one upstream call across concurrent identical cacheable requests.
    pub fn with_single_flight(mut self) -> Self {
        self.single_flight = Some(Arc::new(SingleFlight::new()));
        self
    }

    /// Use a cost table for quota settlement, usage events and metrics.
    pub fn with_cost_calculator(mut self, cost: Arc<dyn ICostCalculator>) -> Self {
        self.cost = cost;
        self
    }

    /// Enable quota reservation/settlement.
    pub fn with_quota(mut self, quota: Arc<dyn IQuotaStore>) -> Self {
        self.quota = Some(quota);
        self
    }

    /// Enable usage events.
    pub fn with_usage(mut self, usage: Arc<dyn IUsageSink>) -> Self {
        self.usage = Some(usage);
        self
    }

    /// Replace the tokenizer used for estimates.
    pub fn with_tokenizer(mut self, tokenizer: Arc<dyn ITokenizer>) -> Self {
        self.tokenizer = tokenizer;
        self
    }

    /// Feed provider outcomes into routing health.
    pub fn with_health(mut self, health: Arc<HealthRegistry>) -> Self {
        self.health = Some(health);
        self
    }

    /// Configure retry/backoff.
    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Enable a per-model circuit breaker.
    pub fn with_circuit_breaker(mut self, circuit: Arc<CircuitBreaker>) -> Self {
        self.circuit = Some(circuit);
        self
    }

    /// Enable a per-provider concurrency bulkhead.
    pub fn with_bulkhead(mut self, bulkhead: Arc<Bulkhead>) -> Self {
        self.bulkhead = Some(bulkhead);
        self
    }

    /// Emit gateway metrics through `meter`.
    pub fn with_meter(mut self, meter: Arc<dyn IMeter>) -> Self {
        self.meter = Some(meter);
        self
    }

    /// Default completion bound when `max_tokens` is absent.
    pub fn with_default_max_tokens(mut self, max_tokens: u32) -> Self {
        self.default_max_tokens = max_tokens.max(1);
        self
    }

    /// Non-streamed execution.
    pub async fn complete(
        &self,
        ctx: &GatewayContext,
        request: CanonicalRequest,
    ) -> Result<CanonicalResponse, GatewayError> {
        let started = Instant::now();
        let cache_key = request_cache_key(&ctx.tenant_id, &request);
        if let (Some(cache), Some(key)) = (&self.cache, cache_key.as_ref()) {
            if let Some(response) = cache.get(key).await {
                self.record_cache_hit(&ctx.model_alias);
                tracing::debug!(
                    request_id = %ctx.request_id,
                    model = %ctx.model_alias,
                    "gateway cache hit"
                );
                record_request_metrics(
                    self.meter.as_ref(),
                    &ctx.model_alias,
                    "complete",
                    "cache_hit",
                    elapsed_ms(started),
                    None,
                    Usage::ZERO,
                );
                return Ok(response);
            }
            self.record_cache_miss(&ctx.model_alias);
        }

        if let (Some(flight), Some(key)) = (&self.single_flight, cache_key.as_ref()) {
            let cell = flight.cell(key);
            let result = cell
                .get_or_init(|| self.complete_uncached(ctx, request.clone(), cache_key.clone()))
                .await
                .clone();
            flight.finish(key, &cell);
            return result;
        }

        self.complete_uncached(ctx, request, cache_key).await
    }

    async fn complete_uncached(
        &self,
        ctx: &GatewayContext,
        request: CanonicalRequest,
        cache_key: Option<String>,
    ) -> Result<CanonicalResponse, GatewayError> {
        let started = Instant::now();
        let plan = self
            .router
            .plan(
                &RouteRequest::new(&ctx.model_alias)
                    .require(required_capabilities(&request, false)),
            )
            .await?;
        let prompt_tokens = estimate_prompt_tokens(&*self.tokenizer, &ctx.model_alias, &request);
        let max_tokens = request.sampling.max_tokens;
        let estimate =
            prompt_tokens.saturating_add(u64::from(max_tokens.unwrap_or(self.default_max_tokens)));
        let est_cost = self.estimated_cost(ctx, prompt_tokens, max_tokens);
        let lease_ttl =
            plan.deadline.saturating_duration_since(Instant::now()) + Duration::from_secs(60);
        let lease_ttl = lease_ttl.max(crate::quota::DEFAULT_LEASE_TTL);
        let mut reservation = match self.reserve(ctx, estimate, 0, est_cost, lease_ttl).await {
            Ok(reservation) => ReservationGuard::new(&self.quota, reservation),
            Err(error) => {
                tracing::warn!(
                    request_id = %ctx.request_id,
                    model = %ctx.model_alias,
                    error = %error,
                    "gateway request rejected by quota"
                );
                self.record_quota_rejection(&ctx.model_alias);
                return Err(error);
            }
        };
        let mut last_error: Option<ProviderError> = None;
        let max_attempts = self.retry.max_attempts().min(plan.attempts.len()).max(1);

        for (index, attempt) in plan.attempts.into_iter().take(max_attempts).enumerate() {
            if Instant::now() >= plan.deadline {
                last_error = Some(ProviderError::Timeout);
                break;
            }
            if let Some(circuit) = &self.circuit {
                if !circuit.allow(&attempt.model) {
                    record_circuit_state(
                        self.meter.as_ref(),
                        &attempt.model,
                        circuit.state(&attempt.model),
                    );
                    last_error = Some(ProviderError::Unavailable("circuit breaker is open".into()));
                    continue;
                }
            }
            let catalog = self.current_catalog();
            let Some(provider) = catalog.provider(&attempt.model.provider) else {
                if let Some(circuit) = &self.circuit {
                    circuit.release_probe(&attempt.model);
                }
                self.release(reservation.take()).await;
                return Err(GatewayError::NoProvider(attempt.model.provider.clone()));
            };
            let _permit = match &self.bulkhead {
                Some(bulkhead) => match bulkhead.acquire(&attempt.model.provider).await {
                    Ok(permit) => Some(permit),
                    Err(error) => {
                        if let Some(circuit) = &self.circuit {
                            circuit.release_probe(&attempt.model);
                        }
                        last_error = Some(ProviderError::Unavailable(error.to_string()));
                        continue;
                    }
                },
                None => None,
            };
            let mut upstream = request.clone();
            upstream.model = attempt.model.model.clone();

            let outcome =
                match tokio::time::timeout(attempt.timeout, provider.complete(upstream)).await {
                    Ok(result) => result,
                    Err(_) => Err(ProviderError::Timeout),
                };

            match outcome {
                Ok(response) => {
                    if let Some(circuit) = &self.circuit {
                        circuit.record_success(&attempt.model);
                        record_circuit_state(
                            self.meter.as_ref(),
                            &attempt.model,
                            circuit.state(&attempt.model),
                        );
                    }
                    if let Some(health) = &self.health {
                        health.record_success(&attempt.model, started.elapsed());
                    }
                    // A completed provider response without a usage block must
                    // not become free usage: settle with the reserved
                    // prompt+completion estimate and flag it in the log.
                    let usage = if response.usage.is_empty() {
                        tracing::warn!(
                            request_id = %ctx.request_id,
                            provider = %attempt.model.provider,
                            "provider omitted usage; settling with the estimate"
                        );
                        Usage::new(
                            prompt_tokens,
                            u64::from(max_tokens.unwrap_or(self.default_max_tokens)),
                        )
                    } else {
                        response.usage
                    };
                    let cost = self.cost.cost_micros(&ctx.model_alias, &usage);
                    self.settle(reservation.take(), usage, cost).await;
                    self.record_usage(
                        ctx,
                        &attempt.model,
                        usage,
                        UsageStatus::Completed,
                        started,
                        None,
                        cost,
                    )
                    .await;
                    record_cost_metric(self.meter.as_ref(), &ctx.model_alias, cost);
                    tracing::info!(
                        request_id = %ctx.request_id,
                        tenant_id = %ctx.tenant_id,
                        model = %ctx.model_alias,
                        provider = %attempt.model.provider,
                        mode = "complete",
                        status = "ok",
                        latency_ms = elapsed_ms(started),
                        prompt_tokens = usage.prompt_tokens,
                        completion_tokens = usage.completion_tokens,
                        cached_tokens = usage.cached_tokens,
                        reasoning_tokens = usage.reasoning_tokens,
                        cost_micros = cost,
                        trace_id = %trace_id(),
                        "gateway request"
                    );
                    record_request_metrics(
                        self.meter.as_ref(),
                        &ctx.model_alias,
                        "complete",
                        "ok",
                        elapsed_ms(started),
                        None,
                        usage,
                    );
                    if let (Some(cache), Some(key)) = (&self.cache, cache_key.as_ref()) {
                        cache
                            .put(key.clone(), response.clone(), self.cache_ttl)
                            .await;
                    }
                    return Ok(response);
                }
                Err(error) => {
                    if error.is_upstream_fault() {
                        if let Some(circuit) = &self.circuit {
                            circuit.record_failure(&attempt.model);
                            record_circuit_state(
                                self.meter.as_ref(),
                                &attempt.model,
                                circuit.state(&attempt.model),
                            );
                        }
                        if let Some(health) = &self.health {
                            health.record_failure(&attempt.model);
                        }
                    } else if let Some(circuit) = &self.circuit {
                        // A client-caused error must not close a half-open
                        // probe against a healthy upstream.
                        circuit.release_probe(&attempt.model);
                    }
                    self.record_upstream_error(&attempt.model.provider, &error);
                    tracing::warn!(
                        request_id = %ctx.request_id,
                        provider = %attempt.model.provider,
                        retryable = error.is_retryable(),
                        error = %error,
                        "upstream attempt failed"
                    );
                    let retry = self.retry.should_retry(index + 1, &error);
                    let delay = self.retry.delay(index + 1, error.retry_after());
                    last_error = Some(error);
                    if !retry {
                        break;
                    }
                    if Instant::now()
                        .checked_add(delay)
                        .is_some_and(|next| next >= plan.deadline)
                    {
                        break;
                    }
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        self.release(reservation.take()).await;
        let error_text = last_error
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        tracing::warn!(
            request_id = %ctx.request_id,
            tenant_id = %ctx.tenant_id,
            model = %ctx.model_alias,
            status = "error",
            latency_ms = elapsed_ms(started),
            error = %error_text,
            trace_id = %trace_id(),
            "gateway request failed"
        );
        record_request_metrics(
            self.meter.as_ref(),
            &ctx.model_alias,
            "complete",
            "error",
            elapsed_ms(started),
            None,
            Usage::ZERO,
        );
        Err(GatewayError::Provider(last_error.unwrap_or_else(|| {
            ProviderError::Unavailable("no provider attempt succeeded".into())
        })))
    }

    /// Streamed execution.
    ///
    /// The returned stream owns the quota reservation and usage sink until it
    /// completes or is dropped; settlement runs in a background task.
    pub async fn stream(
        &self,
        ctx: &GatewayContext,
        request: CanonicalRequest,
    ) -> Result<GatewayStream, GatewayError> {
        let started = Instant::now();
        let plan = self
            .router
            .plan(
                &RouteRequest::new(&ctx.model_alias).require(required_capabilities(&request, true)),
            )
            .await?;
        let prompt_tokens = estimate_prompt_tokens(&*self.tokenizer, &ctx.model_alias, &request);
        let max_tokens = request.sampling.max_tokens;
        let estimate =
            prompt_tokens.saturating_add(u64::from(max_tokens.unwrap_or(self.default_max_tokens)));
        let est_cost = self.estimated_cost(ctx, prompt_tokens, max_tokens);
        let lease_ttl =
            plan.deadline.saturating_duration_since(Instant::now()) + Duration::from_secs(60);
        let lease_ttl = lease_ttl.max(crate::quota::DEFAULT_LEASE_TTL);
        let mut reservation = match self.reserve(ctx, estimate, 1, est_cost, lease_ttl).await {
            Ok(reservation) => ReservationGuard::new(&self.quota, reservation),
            Err(error) => {
                self.record_quota_rejection(&ctx.model_alias);
                return Err(error);
            }
        };
        let mut last_error: Option<ProviderError> = None;
        let max_attempts = self.retry.max_attempts().min(plan.attempts.len()).max(1);
        let stream_deadline = plan.deadline;

        for (index, attempt) in plan.attempts.into_iter().take(max_attempts).enumerate() {
            if Instant::now() >= plan.deadline {
                last_error = Some(ProviderError::Timeout);
                break;
            }
            if let Some(circuit) = &self.circuit {
                if !circuit.allow(&attempt.model) {
                    record_circuit_state(
                        self.meter.as_ref(),
                        &attempt.model,
                        circuit.state(&attempt.model),
                    );
                    last_error = Some(ProviderError::Unavailable("circuit breaker is open".into()));
                    continue;
                }
            }
            let catalog = self.current_catalog();
            let Some(provider) = catalog.provider(&attempt.model.provider) else {
                if let Some(circuit) = &self.circuit {
                    circuit.release_probe(&attempt.model);
                }
                self.release(reservation.take()).await;
                return Err(GatewayError::NoProvider(attempt.model.provider.clone()));
            };
            let permit = match &self.bulkhead {
                Some(bulkhead) => match bulkhead.acquire(&attempt.model.provider).await {
                    Ok(permit) => Some(permit),
                    Err(error) => {
                        if let Some(circuit) = &self.circuit {
                            circuit.release_probe(&attempt.model);
                        }
                        last_error = Some(ProviderError::Unavailable(error.to_string()));
                        continue;
                    }
                },
                None => None,
            };
            let mut upstream = request.clone();
            upstream.model = attempt.model.model.clone();

            let outcome =
                match tokio::time::timeout(attempt.timeout, provider.stream(upstream)).await {
                    Ok(result) => result,
                    Err(_) => Err(ProviderError::Timeout),
                };

            match outcome {
                Ok(stream) => {
                    tracing::info!(
                        request_id = %ctx.request_id,
                        model = %ctx.model_alias,
                        provider = %attempt.model.provider,
                        mode = "stream",
                        status = "started",
                        "gateway stream started"
                    );
                    let active_streams = self.meter.as_ref().map(|meter| {
                        let gauge = meter.gauge(
                            "ai_gateway_active_streams",
                            &[("model", ctx.model_alias.as_str())],
                        );
                        gauge.add(1.0);
                        gauge
                    });
                    return Ok(GatewayStream::new(
                        stream,
                        ctx.clone(),
                        attempt.model,
                        started,
                        stream_deadline,
                        prompt_tokens,
                        reservation.take(),
                        self.quota.clone(),
                        self.usage.clone(),
                        self.tokenizer.clone(),
                        self.health.clone(),
                        permit,
                        self.circuit.clone(),
                        self.meter.clone(),
                        active_streams,
                        self.cost.clone(),
                    ));
                }
                Err(error) => {
                    if error.is_upstream_fault() {
                        if let Some(circuit) = &self.circuit {
                            circuit.record_failure(&attempt.model);
                            record_circuit_state(
                                self.meter.as_ref(),
                                &attempt.model,
                                circuit.state(&attempt.model),
                            );
                        }
                        if let Some(health) = &self.health {
                            health.record_failure(&attempt.model);
                        }
                    } else if let Some(circuit) = &self.circuit {
                        // A client-caused error must not close a half-open
                        // probe against a healthy upstream.
                        circuit.release_probe(&attempt.model);
                    }
                    self.record_upstream_error(&attempt.model.provider, &error);
                    tracing::warn!(
                        request_id = %ctx.request_id,
                        provider = %attempt.model.provider,
                        retryable = error.is_retryable(),
                        error = %error,
                        "upstream attempt failed"
                    );
                    let retry = self.retry.should_retry(index + 1, &error);
                    let delay = self.retry.delay(index + 1, error.retry_after());
                    last_error = Some(error);
                    if !retry {
                        break;
                    }
                    if Instant::now()
                        .checked_add(delay)
                        .is_some_and(|next| next >= plan.deadline)
                    {
                        break;
                    }
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        self.release(reservation.take()).await;
        let error_text = last_error
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        tracing::warn!(
            request_id = %ctx.request_id,
            tenant_id = %ctx.tenant_id,
            model = %ctx.model_alias,
            status = "error",
            latency_ms = elapsed_ms(started),
            error = %error_text,
            trace_id = %trace_id(),
            "gateway request failed"
        );
        record_request_metrics(
            self.meter.as_ref(),
            &ctx.model_alias,
            "stream",
            "error",
            elapsed_ms(started),
            None,
            Usage::ZERO,
        );
        Err(GatewayError::Provider(last_error.unwrap_or_else(|| {
            ProviderError::Unavailable("no provider attempt succeeded".into())
        })))
    }

    fn record_cache_hit(&self, model: &str) {
        let Some(meter) = &self.meter else {
            return;
        };
        meter
            .counter("ai_gateway_cache_hits_total", &[("model", model)])
            .inc();
    }

    fn record_cache_miss(&self, model: &str) {
        let Some(meter) = &self.meter else {
            return;
        };
        meter
            .counter("ai_gateway_cache_misses_total", &[("model", model)])
            .inc();
    }

    fn record_upstream_error(&self, provider: &str, error: &ProviderError) {
        let Some(meter) = &self.meter else {
            return;
        };
        meter
            .counter(
                "ai_gateway_upstream_errors_total",
                &[
                    ("provider", provider),
                    (
                        "retryable",
                        if error.is_retryable() {
                            "true"
                        } else {
                            "false"
                        },
                    ),
                ],
            )
            .inc();
    }

    fn record_quota_rejection(&self, model: &str) {
        let Some(meter) = &self.meter else {
            return;
        };
        meter
            .counter("ai_gateway_quota_rejections_total", &[("model", model)])
            .inc();
    }

    #[allow(clippy::too_many_arguments)]
    async fn reserve(
        &self,
        ctx: &GatewayContext,
        estimated_tokens: u64,
        streams: u32,
        estimated_cost_micros: u64,
        lease_ttl: Duration,
    ) -> Result<Option<Reservation>, GatewayError> {
        let Some(store) = &self.quota else {
            return Ok(None);
        };
        let reservation = store
            .reserve(
                ReserveRequest::new(
                    QuotaKey::new(&ctx.tenant_id, &ctx.key_id, &ctx.model_alias),
                    estimated_tokens,
                )
                .streams(streams)
                .estimated_cost_micros(estimated_cost_micros)
                .lease_ttl(lease_ttl),
            )
            .await?;
        Ok(Some(reservation))
    }

    /// Cost of the worst-case estimate (prompt + max completion tokens), used
    /// for the up-front budget pre-check so `budget_micros` actually enforces a
    /// cap instead of only recording spend after the fact.
    fn estimated_cost(
        &self,
        ctx: &GatewayContext,
        prompt_tokens: u64,
        max_tokens: Option<u32>,
    ) -> u64 {
        let max_completion = u64::from(max_tokens.unwrap_or(self.default_max_tokens));
        let usage = Usage::new(prompt_tokens, max_completion);
        self.cost.cost_micros(&ctx.model_alias, &usage)
    }

    async fn settle(&self, reservation: Option<Reservation>, usage: Usage, cost_micros: u64) {
        if let (Some(store), Some(reservation)) = (&self.quota, reservation) {
            if let Err(error) = store.settle(reservation, usage, cost_micros).await {
                tracing::warn!(
                    error = %error,
                    "quota settlement failed; the lease will expire via TTL"
                );
            }
        }
    }

    async fn release(&self, reservation: Option<Reservation>) {
        if let (Some(store), Some(reservation)) = (&self.quota, reservation) {
            if let Err(error) = store.release(reservation).await {
                tracing::warn!(
                    error = %error,
                    "quota release failed; the lease will expire via TTL"
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_usage(
        &self,
        ctx: &GatewayContext,
        model: &ModelKey,
        usage: Usage,
        status: UsageStatus,
        started: Instant,
        ttft_ms: Option<u32>,
        cost_micros: u64,
    ) {
        let Some(sink) = &self.usage else {
            return;
        };
        let mut event = UsageEvent::new(
            &ctx.request_id,
            &ctx.tenant_id,
            &ctx.key_id,
            &ctx.model_alias,
            &model.provider,
            usage,
            status,
        )
        .with_cost(cost_micros)
        .with_latency(elapsed_ms(started));
        if let Some(ttft_ms) = ttft_ms {
            event = event.with_ttft(ttft_ms);
        }
        let _ = sink.record(event).await;
    }
}

/// A streamed gateway response.
///
/// It forwards canonical events and, on completion/error/drop, settles the quota
/// reservation and records one usage event. Settlement is spawned because
/// `Drop` cannot await.
pub struct GatewayStream {
    inner: CanonicalStream,
    ctx: GatewayContext,
    model: ModelKey,
    tokenizer: Arc<dyn ITokenizer>,
    quota: Option<Arc<dyn IQuotaStore>>,
    reservation: Option<Reservation>,
    usage_sink: Option<Arc<dyn IUsageSink>>,
    health: Option<Arc<HealthRegistry>>,
    _bulkhead_permit: Option<BulkheadPermit>,
    circuit: Option<Arc<CircuitBreaker>>,
    meter: Option<Arc<dyn IMeter>>,
    active_streams: Option<Arc<dyn Gauge>>,
    cost: Arc<dyn ICostCalculator>,
    prompt_tokens: u64,
    usage: Option<Usage>,
    text: String,
    ttft_ms: Option<u32>,
    started_at: Instant,
    /// Overall deadline watchdog. Polled on every `poll_next` so a provider
    /// that emits headers then stalls cannot hold the bulkhead permit and quota
    /// lease forever.
    deadline_sleep: Pin<Box<tokio::time::Sleep>>,
    /// Set when the gateway-side time budget expires. A timeout is our own
    /// deadline, not a provider fault, so `finish` must not record it as a
    /// health/circuit failure against an otherwise healthy provider.
    deadline_hit: bool,
    saw_done: bool,
    finished: bool,
}

impl GatewayStream {
    #[allow(clippy::too_many_arguments)]
    fn new(
        inner: CanonicalStream,
        ctx: GatewayContext,
        model: ModelKey,
        started_at: Instant,
        deadline: Instant,
        prompt_tokens: u64,
        reservation: Option<Reservation>,
        quota: Option<Arc<dyn IQuotaStore>>,
        usage_sink: Option<Arc<dyn IUsageSink>>,
        tokenizer: Arc<dyn ITokenizer>,
        health: Option<Arc<HealthRegistry>>,
        bulkhead_permit: Option<BulkheadPermit>,
        circuit: Option<Arc<CircuitBreaker>>,
        meter: Option<Arc<dyn IMeter>>,
        active_streams: Option<Arc<dyn Gauge>>,
        cost: Arc<dyn ICostCalculator>,
    ) -> Self {
        Self {
            inner,
            ctx,
            model,
            tokenizer,
            quota,
            reservation,
            usage_sink,
            health,
            _bulkhead_permit: bulkhead_permit,
            circuit,
            meter,
            active_streams,
            cost,
            prompt_tokens,
            usage: None,
            text: String::new(),
            ttft_ms: None,
            started_at,
            deadline_sleep: Box::pin(tokio::time::sleep_until(deadline.into())),
            deadline_hit: false,
            saw_done: false,
            finished: false,
        }
    }

    fn current_usage(&self) -> Usage {
        self.usage.unwrap_or_else(|| Usage {
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self
                .tokenizer
                .count(&self.model.model, &self.text)
                .try_into()
                .unwrap_or(u64::MAX),
            ..Usage::ZERO
        })
    }

    fn finish(&mut self, status: UsageStatus) {
        if self.finished {
            return;
        }
        self.finished = true;

        // A gateway-side deadline is not a provider fault: do not penalize a
        // healthy (just slow) provider in the health registry or circuit. Still
        // settle usage as UpstreamError so the request is not billed as success.
        let provider_at_fault = status == UsageStatus::UpstreamError && !self.deadline_hit;
        if let Some(health) = &self.health {
            match status {
                UsageStatus::Completed => {
                    health.record_success(&self.model, self.started_at.elapsed())
                }
                UsageStatus::UpstreamError if provider_at_fault => {
                    health.record_failure(&self.model)
                }
                _ => {}
            }
        }
        if let Some(circuit) = &self.circuit {
            match status {
                UsageStatus::Completed | UsageStatus::ClientGone => {
                    circuit.record_success(&self.model)
                }
                UsageStatus::UpstreamError if provider_at_fault => {
                    circuit.record_failure(&self.model)
                }
                // Timeout / client-gone: leave the circuit state untouched.
                _ => {}
            }
            record_circuit_state(self.meter.as_ref(), &self.model, circuit.state(&self.model));
        }

        let usage = self.current_usage();
        let cost = self.cost.cost_micros(&self.ctx.model_alias, &usage);
        if let Some(gauge) = &self.active_streams {
            gauge.add(-1.0);
        }
        record_request_metrics(
            self.meter.as_ref(),
            &self.ctx.model_alias,
            "stream",
            usage_status_name(status),
            elapsed_ms(self.started_at),
            self.ttft_ms,
            usage,
        );
        record_cost_metric(self.meter.as_ref(), &self.ctx.model_alias, cost);
        tracing::info!(
            request_id = %self.ctx.request_id,
            tenant_id = %self.ctx.tenant_id,
            model = %self.ctx.model_alias,
            provider = %self.model.provider,
            mode = "stream",
            status = usage_status_name(status),
            latency_ms = elapsed_ms(self.started_at),
            ttft_ms = ?self.ttft_ms,
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            cached_tokens = usage.cached_tokens,
            reasoning_tokens = usage.reasoning_tokens,
            cost_micros = cost,
            trace_id = %trace_id(),
            "gateway stream finished"
        );
        // Check for a runtime *before* taking ownership: if there is none, keep
        // the reservation/sink in `self` so they are dropped with the stream
        // (the lease then expires via TTL) rather than being silently stranded.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                request_id = %self.ctx.request_id,
                "stream settled without a tokio runtime; quota lease will expire via TTL"
            );
            return;
        };

        let quota = self.quota.take();
        let reservation = self.reservation.take();
        let usage_sink = self.usage_sink.take();
        if quota.is_none() && usage_sink.is_none() {
            return;
        }

        let mut event = UsageEvent::new(
            &self.ctx.request_id,
            &self.ctx.tenant_id,
            &self.ctx.key_id,
            &self.ctx.model_alias,
            &self.model.provider,
            usage,
            status,
        )
        .with_cost(cost)
        .with_latency(elapsed_ms(self.started_at));
        if let Some(ttft_ms) = self.ttft_ms {
            event = event.with_ttft(ttft_ms);
        }
        let request_id = self.ctx.request_id.clone();

        handle.spawn(async move {
            if let (Some(store), Some(reservation)) = (quota, reservation) {
                if let Err(error) = store.settle(reservation, usage, cost).await {
                    tracing::warn!(request_id = %request_id, error = %error, "deferred quota settlement failed");
                }
            }
            if let Some(sink) = usage_sink {
                if let Err(error) = sink.record(event).await {
                    tracing::warn!(request_id = %request_id, error = %error, "deferred usage record failed");
                }
            }
        });
    }
}

impl Stream for GatewayStream {
    type Item = Result<StreamEvent, GatewayError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.finished {
            return Poll::Ready(None);
        }

        // Poll the inner stream first: if the provider has already buffered a
        // Done/Usage (or any delta), deliver it even if the deadline has just
        // elapsed. Only when the provider is genuinely idle do we enforce the
        // overall deadline, tearing down the stream to free the permit + lease.
        // Polling the timer here also registers its waker, so a stalled stream
        // is still woken when the deadline fires.
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(event))) => match event {
                StreamEvent::Start { id, model } => {
                    Poll::Ready(Some(Ok(StreamEvent::Start { id, model })))
                }
                StreamEvent::Delta(delta) => {
                    if this.ttft_ms.is_none() && !delta.is_empty() {
                        this.ttft_ms = Some(elapsed_ms(this.started_at));
                    }
                    if let Some(content) = &delta.content {
                        this.text.push_str(content);
                    }
                    if let Some(reasoning) = &delta.reasoning {
                        this.text.push_str(reasoning);
                    }
                    for tool_call in &delta.tool_calls {
                        if let Some(arguments) = &tool_call.arguments_delta {
                            this.text.push_str(arguments);
                        }
                    }
                    Poll::Ready(Some(Ok(StreamEvent::Delta(delta))))
                }
                StreamEvent::Usage(usage) => {
                    this.usage = Some(usage);
                    Poll::Ready(Some(Ok(StreamEvent::Usage(usage))))
                }
                StreamEvent::Done(reason) => {
                    this.saw_done = true;
                    Poll::Ready(Some(Ok(StreamEvent::Done(reason))))
                }
                StreamEvent::Error(error) => {
                    this.finish(UsageStatus::UpstreamError);
                    Poll::Ready(Some(Err(GatewayError::Provider(error))))
                }
            },
            Poll::Ready(Some(Err(error))) => {
                this.finish(UsageStatus::UpstreamError);
                Poll::Ready(Some(Err(GatewayError::Provider(error))))
            }
            Poll::Ready(None) => {
                this.finish(UsageStatus::Completed);
                Poll::Ready(None)
            }
            Poll::Pending => {
                if this.deadline_sleep.as_mut().poll(cx).is_ready() {
                    this.deadline_hit = true;
                    this.finish(UsageStatus::UpstreamError);
                    Poll::Ready(Some(Err(GatewayError::Provider(ProviderError::Timeout))))
                } else {
                    Poll::Pending
                }
            }
        }
    }
}

impl Drop for GatewayStream {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let status = if self.saw_done {
            UsageStatus::Completed
        } else {
            UsageStatus::ClientGone
        };
        self.finish(status);
    }
}

fn required_capabilities(request: &CanonicalRequest, streaming: bool) -> ProviderCapabilities {
    let mut required = ProviderCapabilities::none();
    if streaming {
        required = required.stream();
    }
    if !request.tools.is_empty() {
        required = required.tools();
    }
    if request.messages.iter().any(|message| {
        message
            .content
            .iter()
            .any(|part| matches!(part, ContentPart::Image { .. }))
    }) {
        required = required.vision();
    }
    required
}

fn estimate_prompt_tokens(
    tokenizer: &dyn ITokenizer,
    model: &str,
    request: &CanonicalRequest,
) -> u64 {
    let mut total = 0u64;
    for message in &request.messages {
        for part in &message.content {
            let tokens = match part {
                ContentPart::Text(text) => tokenizer.count(model, text),
                ContentPart::Image { .. } => 0,
                ContentPart::ToolCall(call) => {
                    tokenizer.count(model, &call.name) + tokenizer.count(model, &call.arguments)
                }
                ContentPart::ToolResult { content, .. } => tokenizer.count(model, content),
            };
            total = total.saturating_add(tokens.try_into().unwrap_or(u64::MAX));
        }
    }
    for tool in &request.tools {
        let parameters = serde_json::to_string(&tool.parameters).unwrap_or_default();
        let tokens = tokenizer.count(model, &tool.name)
            + tokenizer.count(model, &tool.description)
            + tokenizer.count(model, &parameters);
        total = total.saturating_add(tokens.try_into().unwrap_or(u64::MAX));
    }
    total
}

fn elapsed_ms(started: Instant) -> u32 {
    started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
}

fn trace_id() -> String {
    webx_core::request_context::RequestContext::trace_id()
        .map(|id| id.to_string())
        .unwrap_or_default()
}

fn usage_status_name(status: UsageStatus) -> &'static str {
    match status {
        UsageStatus::Completed => "ok",
        UsageStatus::ClientGone => "client_gone",
        UsageStatus::UpstreamError => "upstream_error",
    }
}

/// Publish the breaker state as a gauge: 0 = closed, 1 = half-open, 2 = open.
///
/// Labels are provider + upstream model (never tenant/key), matching the other
/// upstream metrics. A dashboard can alert on `max(...) == 2`.
fn record_circuit_state(meter: Option<&Arc<dyn IMeter>>, key: &ModelKey, state: CircuitState) {
    let Some(meter) = meter else {
        return;
    };
    let value = match state {
        CircuitState::Closed => 0.0,
        CircuitState::HalfOpen => 1.0,
        CircuitState::Open => 2.0,
    };
    meter
        .gauge(
            "ai_gateway_circuit_state",
            &[
                ("provider", key.provider.as_str()),
                ("model", key.model.as_str()),
            ],
        )
        .set(value);
}

fn record_cost_metric(meter: Option<&Arc<dyn IMeter>>, model: &str, cost_micros: u64) {
    let Some(meter) = meter else {
        return;
    };
    if cost_micros > 0 {
        meter
            .counter("ai_gateway_cost_micros_total", &[("model", model)])
            .add(cost_micros);
    }
}

fn record_request_metrics(
    meter: Option<&Arc<dyn IMeter>>,
    model: &str,
    mode: &str,
    status: &str,
    latency_ms: u32,
    ttft_ms: Option<u32>,
    usage: Usage,
) {
    let Some(meter) = meter else {
        return;
    };
    let tags = [("model", model), ("mode", mode), ("status", status)];
    meter.counter("ai_gateway_requests_total", &tags).inc();
    meter
        .histogram("ai_gateway_request_duration_ms", &tags)
        .record(f64::from(latency_ms));
    if let Some(ttft_ms) = ttft_ms {
        meter
            .histogram("ai_gateway_ttft_ms", &[("model", model)])
            .record(f64::from(ttft_ms));
    }
    for (kind, value) in [
        ("prompt", usage.prompt_tokens),
        ("completion", usage.completion_tokens),
        ("cached", usage.cached_tokens),
        ("reasoning", usage.reasoning_tokens),
    ] {
        if value > 0 {
            meter
                .counter(
                    "ai_gateway_tokens_total",
                    &[("model", model), ("kind", kind)],
                )
                .add(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Alias, ChatProvider, ModelSpec, RouteStrategy, RouteTarget};
    use crate::quota::{MemoryQuotaStore, QuotaLimits, StaticQuotaPolicy};
    use crate::resilience::{Bulkhead, CircuitBreaker, RetryPolicy};
    use crate::routing::CatalogRouter;
    use crate::usage::{MemoryUsageSink, Usage};
    use futures_util::stream::{self, StreamExt};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use webx_core::metrics::MemoryMeter;

    struct MockProvider {
        id: String,
        capabilities: ProviderCapabilities,
        complete: Mutex<Option<Result<CanonicalResponse, ProviderError>>>,
        stream: Mutex<Option<Vec<Result<StreamEvent, ProviderError>>>>,
        pending_stream: bool,
        calls: AtomicUsize,
    }

    impl MockProvider {
        fn mock(
            id: &str,
            complete: Option<Result<CanonicalResponse, ProviderError>>,
            stream: Option<Vec<Result<StreamEvent, ProviderError>>>,
        ) -> Arc<dyn ChatProvider> {
            Arc::new(Self {
                id: id.into(),
                capabilities: ProviderCapabilities::chat_stream().tools(),
                complete: Mutex::new(complete),
                stream: Mutex::new(stream),
                pending_stream: false,
                calls: AtomicUsize::new(0),
            })
        }

        fn pending(
            id: &str,
            stream: Option<Vec<Result<StreamEvent, ProviderError>>>,
        ) -> Arc<dyn ChatProvider> {
            Arc::new(Self {
                id: id.into(),
                capabilities: ProviderCapabilities::chat_stream().tools(),
                complete: Mutex::new(None),
                stream: Mutex::new(stream),
                pending_stream: true,
                calls: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait::async_trait]
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
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.complete
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
                .unwrap_or_else(|| Err(ProviderError::Unavailable("no script".into())))
        }

        async fn stream(
            &self,
            _request: CanonicalRequest,
        ) -> Result<CanonicalStream, ProviderError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let events = self
                .stream
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            match events {
                Some(events) => {
                    let stream = stream::iter(events);
                    if self.pending_stream {
                        Ok(stream.chain(stream::pending()).boxed())
                    } else {
                        Ok(stream.boxed())
                    }
                }
                None => Err(ProviderError::Unavailable("no script".into())),
            }
        }
    }

    fn response() -> CanonicalResponse {
        CanonicalResponse {
            id: "ok".into(),
            model: "model-b".into(),
            message: crate::ir::Message::text(crate::ir::Role::Assistant, "hello"),
            usage: Usage::new(3, 4),
            finish_reason: crate::ir::FinishReason::Stop,
        }
    }

    struct CountingProvider {
        calls: AtomicUsize,
        error: ProviderError,
    }
    impl CountingProvider {
        fn new(error: ProviderError) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                error,
            })
        }
    }

    #[async_trait::async_trait]
    impl ChatProvider for CountingProvider {
        fn id(&self) -> &str {
            "p"
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::chat_stream().tools()
        }

        async fn complete(
            &self,
            _request: CanonicalRequest,
        ) -> Result<CanonicalResponse, ProviderError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Err(self.error.clone())
        }

        async fn stream(
            &self,
            _request: CanonicalRequest,
        ) -> Result<CanonicalStream, ProviderError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Err(self.error.clone())
        }
    }

    struct StaticProvider {
        response: CanonicalResponse,
    }

    impl StaticProvider {
        fn arc(id: &str) -> Arc<dyn ChatProvider> {
            Arc::new(Self {
                response: CanonicalResponse {
                    id: id.into(),
                    model: "model-p".into(),
                    message: crate::ir::Message::text(crate::ir::Role::Assistant, id),
                    usage: Usage::new(1, 1),
                    finish_reason: crate::ir::FinishReason::Stop,
                },
            })
        }
    }

    #[async_trait::async_trait]
    impl ChatProvider for StaticProvider {
        fn id(&self) -> &str {
            "p"
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::chat_stream().tools()
        }

        async fn complete(
            &self,
            _request: CanonicalRequest,
        ) -> Result<CanonicalResponse, ProviderError> {
            Ok(self.response.clone())
        }

        async fn stream(
            &self,
            _request: CanonicalRequest,
        ) -> Result<CanonicalStream, ProviderError> {
            Err(ProviderError::Unsupported("not scripted".into()))
        }
    }

    struct CountingSuccessProvider {
        calls: AtomicUsize,
        delay: Duration,
    }

    #[async_trait::async_trait]
    impl ChatProvider for CountingSuccessProvider {
        fn id(&self) -> &str {
            "p"
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::chat_stream().tools()
        }

        async fn complete(
            &self,
            _request: CanonicalRequest,
        ) -> Result<CanonicalResponse, ProviderError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            Ok(CanonicalResponse {
                id: "cached".into(),
                model: "model-p".into(),
                message: crate::ir::Message::text(crate::ir::Role::Assistant, "cached"),
                usage: Usage::new(2, 2),
                finish_reason: crate::ir::FinishReason::Stop,
            })
        }

        async fn stream(
            &self,
            _request: CanonicalRequest,
        ) -> Result<CanonicalStream, ProviderError> {
            Err(ProviderError::Unsupported("not scripted".into()))
        }
    }

    fn catalog(a: Arc<dyn ChatProvider>, b: Arc<dyn ChatProvider>) -> Arc<ModelCatalog> {
        let mut catalog = ModelCatalog::new(1);
        catalog.add_provider(a);
        catalog.add_provider(b);
        let key_a = ModelKey::new("a", "model-a");
        let key_b = ModelKey::new("b", "model-b");
        catalog.add_model(
            ModelSpec::new(key_a.clone())
                .with_capabilities(ProviderCapabilities::chat_stream().tools()),
        );
        catalog.add_model(
            ModelSpec::new(key_b.clone())
                .with_capabilities(ProviderCapabilities::chat_stream().tools()),
        );
        catalog.add_alias(
            "fast",
            Alias::new(
                RouteStrategy::Fallback,
                vec![RouteTarget::new(key_a), RouteTarget::new(key_b)],
            ),
        );
        Arc::new(catalog)
    }

    fn catalog_single(provider: Arc<dyn ChatProvider>) -> Arc<ModelCatalog> {
        catalog_version(provider, 1)
    }

    fn catalog_version(provider: Arc<dyn ChatProvider>, version: u64) -> Arc<ModelCatalog> {
        let mut catalog = ModelCatalog::new(version);
        catalog.add_provider(provider);
        let key = ModelKey::new("p", "model-p");
        catalog.add_model(
            ModelSpec::new(key.clone())
                .with_capabilities(ProviderCapabilities::chat_stream().tools()),
        );
        catalog.add_alias(
            "fast",
            Alias::new(RouteStrategy::Fallback, vec![RouteTarget::new(key)]),
        );
        Arc::new(catalog)
    }

    fn quota(limits: QuotaLimits) -> Arc<MemoryQuotaStore> {
        Arc::new(MemoryQuotaStore::new(Arc::new(StaticQuotaPolicy::new(
            limits,
        ))))
    }

    fn context() -> GatewayContext {
        GatewayContext::new("req-1", "tenant-1", "key-1", "fast")
    }

    fn request() -> CanonicalRequest {
        CanonicalRequest::new("seed")
            .with_messages(vec![crate::ir::Message::text(crate::ir::Role::User, "hi")])
    }

    #[tokio::test]
    async fn complete_retries_a_retryable_error() {
        let a = MockProvider::mock(
            "a",
            Some(Err(ProviderError::Unavailable("down".into()))),
            None,
        );
        let b = MockProvider::mock("b", Some(Ok(response())), None);
        let catalog = catalog(a, b);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let health = Arc::clone(router.health());
        let sink = Arc::new(MemoryUsageSink::new());
        let executor = GatewayExecutor::new(catalog, router)
            .with_health(health)
            .with_usage(Arc::clone(&sink) as Arc<dyn IUsageSink>);

        let result = executor.complete(&context(), request()).await;
        assert_eq!(result.expect("second provider succeeds").id, "ok");
        assert_eq!(sink.len(), 1);
        assert_eq!(
            sink.get("req-1").expect("usage recorded").status,
            UsageStatus::Completed
        );
    }

    #[tokio::test]
    async fn complete_returns_provider_error_when_all_attempts_fail() {
        let a = MockProvider::mock(
            "a",
            Some(Err(ProviderError::Unavailable("down".into()))),
            None,
        );
        let b = MockProvider::mock("b", Some(Err(ProviderError::Protocol("bad".into()))), None);
        let catalog = catalog(a, b);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let executor = GatewayExecutor::new(catalog, router);

        let error = executor
            .complete(&context(), request())
            .await
            .expect_err("all attempts fail");
        assert!(matches!(error, GatewayError::Provider(_)));
        assert_eq!(error.http_status(), 502);
    }

    #[tokio::test]
    async fn quota_exceeded_skips_the_provider() {
        let a = MockProvider::mock("a", Some(Ok(response())), None);
        let b = MockProvider::mock("b", Some(Ok(response())), None);
        let catalog = catalog(a, b);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let store = quota(QuotaLimits::default().requests_per_minute(0));
        let executor = GatewayExecutor::new(catalog, router).with_quota(store);

        let error = executor
            .complete(&context(), request())
            .await
            .expect_err("quota rejects");
        assert!(matches!(error, GatewayError::Quota(_)));
        assert_eq!(error.http_status(), 429);
    }

    #[tokio::test]
    async fn stream_records_usage_after_completion() {
        let a = MockProvider::mock("a", None, None);
        let b = MockProvider::mock(
            "b",
            None,
            Some(vec![
                Ok(StreamEvent::Start {
                    id: "ok".into(),
                    model: "model-b".into(),
                }),
                Ok(StreamEvent::Delta(crate::ir::Delta::text("hello"))),
                Ok(StreamEvent::Usage(Usage::new(2, 3))),
                Ok(StreamEvent::Done(crate::ir::FinishReason::Stop)),
            ]),
        );
        let catalog = catalog(a, b);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let sink = Arc::new(MemoryUsageSink::new());
        let store = quota(QuotaLimits::default());
        let executor = GatewayExecutor::new(catalog, router)
            .with_quota(store)
            .with_usage(Arc::clone(&sink) as Arc<dyn IUsageSink>);

        let stream = executor
            .stream(&context(), request().stream())
            .await
            .expect("stream starts");
        let events: Vec<_> = stream.collect().await;
        assert_eq!(events.len(), 4);
        assert!(events.iter().all(|event| event.is_ok()));

        tokio::time::sleep(Duration::from_millis(20)).await;
        let event = sink.get("req-1").expect("usage recorded");
        assert_eq!(event.status, UsageStatus::Completed);
        assert_eq!(event.usage.total_tokens(), 5);
        assert!(event.ttft_ms.is_some());
    }

    #[tokio::test]
    async fn stream_deadline_fires_when_provider_stalls_after_headers() {
        let provider = MockProvider::pending(
            "p",
            Some(vec![Ok(StreamEvent::Start {
                id: "stalled".into(),
                model: "model-p".into(),
            })]),
        );
        let catalog = catalog_version(provider as Arc<dyn ChatProvider>, 1);
        let router = Arc::new(
            CatalogRouter::new(Arc::clone(&catalog))
                .with_default_timeout(Duration::from_millis(40)),
        );
        let executor = GatewayExecutor::new(catalog, router);

        let mut stream = executor
            .stream(&context(), request())
            .await
            .expect("stream starts");
        // First event is the Start frame; the next poll must hit the deadline
        // rather than hanging forever on `stream::pending()`.
        let first = stream.next().await.expect("start frame");
        assert!(first.is_ok());
        let timed = tokio::time::timeout(Duration::from_millis(500), stream.next()).await;
        let timed = timed.expect("deadline must resolve the stalled stream");
        assert!(
            matches!(
                timed,
                Some(Err(GatewayError::Provider(ProviderError::Timeout)))
            ),
            "expected a timeout error, got {timed:?}"
        );
    }

    #[tokio::test]
    async fn stream_deadline_does_not_penalize_a_healthy_provider() {
        // A gateway-side deadline is our own time budget, not a provider fault.
        // Stalling the stream must not open the circuit against the provider.
        let provider = MockProvider::pending(
            "p",
            Some(vec![Ok(StreamEvent::Start {
                id: "slow".into(),
                model: "model-p".into(),
            })]),
        );
        let catalog = catalog_single(provider as Arc<dyn ChatProvider>);
        let router = Arc::new(
            CatalogRouter::new(Arc::clone(&catalog))
                .with_default_timeout(Duration::from_millis(30)),
        );
        // Threshold 1 means a single recorded failure would open the circuit.
        let circuit = Arc::new(CircuitBreaker::new(1, Duration::from_secs(60)));
        let executor =
            GatewayExecutor::new(catalog, router).with_circuit_breaker(Arc::clone(&circuit));

        let mut stream = executor
            .stream(&context(), request())
            .await
            .expect("stream starts");
        let _ = stream.next().await.expect("start frame").expect("ok");
        let timed = tokio::time::timeout(Duration::from_millis(500), stream.next())
            .await
            .expect("deadline resolves the stalled stream");
        assert!(
            matches!(
                timed,
                Some(Err(GatewayError::Provider(ProviderError::Timeout)))
            ),
            "expected timeout, got {timed:?}"
        );

        // The timeout must NOT have been counted as a provider failure: the
        // circuit is still closed and allows another attempt through.
        let key = ModelKey::new("p", "model-p");
        assert!(
            circuit.allow(&key),
            "a healthy but slow provider must not be tripped by our deadline"
        );
    }

    #[tokio::test]
    async fn dropping_a_stream_records_client_gone_and_releases_the_slot() {
        let a = MockProvider::mock("a", None, None);
        let b = MockProvider::pending(
            "b",
            Some(vec![
                Ok(StreamEvent::Start {
                    id: "ok".into(),
                    model: "model-b".into(),
                }),
                Ok(StreamEvent::Delta(crate::ir::Delta::text("hello"))),
            ]),
        );
        let catalog = catalog(a, b);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let sink = Arc::new(MemoryUsageSink::new());
        let store = quota(QuotaLimits::default().concurrent_streams(1));
        let executor = GatewayExecutor::new(catalog, router)
            .with_quota(Arc::clone(&store) as Arc<dyn IQuotaStore>)
            .with_usage(Arc::clone(&sink) as Arc<dyn IUsageSink>);

        let mut stream = executor
            .stream(&context(), request().stream())
            .await
            .expect("stream starts");
        let _ = stream.next().await.expect("start event").expect("ok");
        let _ = stream.next().await.expect("delta event").expect("ok");
        drop(stream);

        tokio::time::sleep(Duration::from_millis(20)).await;
        let event = sink.get("req-1").expect("usage recorded");
        assert_eq!(event.status, UsageStatus::ClientGone);

        // The concurrency slot is released after the drop settlement.
        store
            .reserve(ReserveRequest::new(QuotaKey::new("tenant-1", "key-1", "fast"), 0).streams(1))
            .await
            .expect("slot released");
    }

    #[tokio::test]
    async fn stream_creation_retries_the_next_provider() {
        let a = MockProvider::mock("a", None, None);
        let b = MockProvider::mock(
            "b",
            None,
            Some(vec![
                Ok(StreamEvent::Start {
                    id: "ok".into(),
                    model: "model-b".into(),
                }),
                Ok(StreamEvent::Done(crate::ir::FinishReason::Stop)),
            ]),
        );
        let catalog = catalog(a, b);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let executor = GatewayExecutor::new(catalog, router);

        let stream = executor
            .stream(&context(), request().stream())
            .await
            .expect("falls back to b");
        let events: Vec<_> = stream.collect().await;
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| event.is_ok()));
    }

    #[tokio::test]
    async fn circuit_breaker_skips_the_provider_after_threshold() {
        let provider = CountingProvider::new(ProviderError::Unavailable("down".into()));
        let catalog = catalog_single(Arc::clone(&provider) as Arc<dyn ChatProvider>);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let circuit = Arc::new(CircuitBreaker::new(1, Duration::from_secs(60)));
        let executor = GatewayExecutor::new(catalog, router)
            .with_retry_policy(RetryPolicy::immediate())
            .with_circuit_breaker(circuit);

        assert!(executor.complete(&context(), request()).await.is_err());
        assert_eq!(provider.calls.load(Ordering::Relaxed), 1);

        let error = executor
            .complete(&context(), request())
            .await
            .expect_err("circuit is open");
        assert_eq!(error.http_status(), 503);
        assert_eq!(
            provider.calls.load(Ordering::Relaxed),
            1,
            "the provider must not be called while the circuit is open"
        );
    }

    #[tokio::test]
    async fn circuit_state_is_exported_as_a_gauge() {
        let provider = CountingProvider::new(ProviderError::Unavailable("down".into()));
        let catalog = catalog_single(Arc::clone(&provider) as Arc<dyn ChatProvider>);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let circuit = Arc::new(CircuitBreaker::new(1, Duration::from_secs(60)));
        let meter = Arc::new(MemoryMeter::new());
        let executor = GatewayExecutor::new(catalog, router)
            .with_retry_policy(RetryPolicy::immediate())
            .with_circuit_breaker(circuit)
            .with_meter(Arc::clone(&meter) as Arc<dyn IMeter>);

        let tags = [("provider", "p"), ("model", "model-p")];
        assert_eq!(meter.gauge_value("ai_gateway_circuit_state", &tags), 0.0);

        let _ = executor.complete(&context(), request()).await;
        assert_eq!(
            meter.gauge_value("ai_gateway_circuit_state", &tags),
            2.0,
            "an opened circuit is exported as 2"
        );

        // The second request is short-circuited; the gauge stays Open.
        let _ = executor.complete(&context(), request()).await;
        assert_eq!(meter.gauge_value("ai_gateway_circuit_state", &tags), 2.0);
    }

    #[tokio::test]
    async fn bulkhead_rejects_when_the_provider_is_saturated() {
        let provider = CountingProvider::new(ProviderError::Unavailable("down".into()));
        let catalog = catalog_single(Arc::clone(&provider) as Arc<dyn ChatProvider>);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let bulkhead = Arc::new(Bulkhead::new(1, Duration::ZERO));
        let held = bulkhead.acquire("p").await.expect("hold the only slot");
        let executor = GatewayExecutor::new(catalog, router)
            .with_retry_policy(RetryPolicy::immediate())
            .with_bulkhead(bulkhead);

        let error = executor
            .complete(&context(), request())
            .await
            .expect_err("bulkhead is full");
        assert_eq!(error.http_status(), 503);
        assert_eq!(provider.calls.load(Ordering::Relaxed), 0);
        drop(held);
    }

    #[tokio::test]
    async fn complete_records_request_and_token_metrics() {
        let a = MockProvider::mock("a", None, None);
        let b = MockProvider::mock("b", Some(Ok(response())), None);
        let catalog = catalog(a, b);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let meter = Arc::new(MemoryMeter::new());
        let executor = GatewayExecutor::new(catalog, router)
            .with_retry_policy(RetryPolicy::immediate())
            .with_meter(Arc::clone(&meter) as Arc<dyn IMeter>);

        executor.complete(&context(), request()).await.expect("ok");
        assert_eq!(
            meter.counter_value(
                "ai_gateway_requests_total",
                &[("status", "ok"), ("model", "fast"), ("mode", "complete")]
            ),
            1
        );
        assert_eq!(
            meter.counter_value(
                "ai_gateway_tokens_total",
                &[("model", "fast"), ("kind", "prompt")]
            ),
            3
        );
        assert_eq!(
            meter
                .histogram_values(
                    "ai_gateway_request_duration_ms",
                    &[("status", "ok"), ("model", "fast"), ("mode", "complete")]
                )
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn stream_records_ttft_and_active_stream_gauge() {
        let a = MockProvider::mock("a", None, None);
        let b = MockProvider::mock(
            "b",
            None,
            Some(vec![
                Ok(StreamEvent::Start {
                    id: "ok".into(),
                    model: "model-b".into(),
                }),
                Ok(StreamEvent::Delta(crate::ir::Delta::text("hello"))),
                Ok(StreamEvent::Usage(Usage::new(2, 3))),
                Ok(StreamEvent::Done(crate::ir::FinishReason::Stop)),
            ]),
        );
        let catalog = catalog(a, b);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let meter = Arc::new(MemoryMeter::new());
        let executor = GatewayExecutor::new(catalog, router)
            .with_retry_policy(RetryPolicy::immediate())
            .with_meter(Arc::clone(&meter) as Arc<dyn IMeter>);

        let stream = executor
            .stream(&context(), request().stream())
            .await
            .expect("stream starts");
        let _: Vec<_> = stream.collect().await;

        assert_eq!(
            meter.counter_value(
                "ai_gateway_requests_total",
                &[("status", "ok"), ("model", "fast"), ("mode", "stream")]
            ),
            1
        );
        assert_eq!(
            meter
                .histogram_values("ai_gateway_ttft_ms", &[("model", "fast")])
                .len(),
            1
        );
        assert_eq!(
            meter.gauge_value("ai_gateway_active_streams", &[("model", "fast")]),
            0.0
        );
    }

    #[tokio::test]
    async fn quota_rejection_is_counted() {
        let a = MockProvider::mock("a", Some(Ok(response())), None);
        let b = MockProvider::mock("b", None, None);
        let catalog = catalog(a, b);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let store = quota(QuotaLimits::default().requests_per_minute(0));
        let meter = Arc::new(MemoryMeter::new());
        let executor = GatewayExecutor::new(catalog, router)
            .with_quota(store)
            .with_meter(Arc::clone(&meter) as Arc<dyn IMeter>);

        let _ = executor.complete(&context(), request()).await;
        assert_eq!(
            meter.counter_value("ai_gateway_quota_rejections_total", &[("model", "fast")]),
            1
        );
    }

    #[tokio::test]
    async fn hot_catalog_reload_changes_provider_without_restart() {
        let handle = CatalogHandle::new(catalog_version(StaticProvider::arc("old"), 1));
        let router = Arc::new(CatalogRouter::from_handle(handle.clone()));
        let executor = GatewayExecutor::new_with_handle(handle.clone(), router)
            .with_retry_policy(RetryPolicy::immediate());

        let first = executor
            .complete(&context(), request())
            .await
            .expect("first");
        assert_eq!(first.id, "old");

        assert!(handle.reload(catalog_version(StaticProvider::arc("new"), 2)));
        assert_eq!(handle.version(), 2);
        let second = executor
            .complete(&context(), request())
            .await
            .expect("second");
        assert_eq!(second.id, "new");

        assert!(
            !handle.reload(catalog_version(StaticProvider::arc("stale"), 1)),
            "older versions are rejected"
        );
        assert_eq!(handle.version(), 2);
    }

    #[tokio::test]
    async fn exact_cache_reuses_a_deterministic_response() {
        let provider = Arc::new(CountingSuccessProvider {
            calls: AtomicUsize::new(0),
            delay: Duration::ZERO,
        });
        let catalog = catalog_version(Arc::clone(&provider) as Arc<dyn ChatProvider>, 1);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let cache = Arc::new(crate::cache::MemoryGatewayCache::new(8));
        let executor = GatewayExecutor::new(catalog, router)
            .with_retry_policy(RetryPolicy::immediate())
            .with_cache(cache);
        let deterministic = request().with_sampling(crate::ir::SamplingParams {
            temperature: Some(0.0),
            ..crate::ir::SamplingParams::default()
        });

        let first = executor
            .complete(&context(), deterministic.clone())
            .await
            .expect("first");
        let second = executor
            .complete(&context(), deterministic)
            .await
            .expect("cached");
        assert_eq!(first.id, "cached");
        assert_eq!(second.id, "cached");
        assert_eq!(
            provider.calls.load(Ordering::Relaxed),
            1,
            "the second call is a cache hit"
        );

        let warm = request().with_sampling(crate::ir::SamplingParams {
            temperature: Some(0.7),
            ..crate::ir::SamplingParams::default()
        });
        executor
            .complete(&context(), warm.clone())
            .await
            .expect("warm 1");
        executor.complete(&context(), warm).await.expect("warm 2");
        assert_eq!(
            provider.calls.load(Ordering::Relaxed),
            3,
            "temperature > 0 bypasses the cache"
        );
    }

    #[tokio::test]
    async fn single_flight_shares_one_concurrent_upstream_call() {
        let provider = Arc::new(CountingSuccessProvider {
            calls: AtomicUsize::new(0),
            delay: Duration::from_millis(20),
        });
        let catalog = catalog_version(Arc::clone(&provider) as Arc<dyn ChatProvider>, 1);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let executor = Arc::new(
            GatewayExecutor::new(catalog, router)
                .with_retry_policy(RetryPolicy::immediate())
                .with_cache(Arc::new(crate::cache::MemoryGatewayCache::new(8)))
                .with_single_flight(),
        );
        let deterministic = request().with_sampling(crate::ir::SamplingParams {
            temperature: Some(0.0),
            ..crate::ir::SamplingParams::default()
        });

        let first = {
            let executor = Arc::clone(&executor);
            let request = deterministic.clone();
            tokio::spawn(async move { executor.complete(&context(), request).await })
        };
        let second = {
            let executor = Arc::clone(&executor);
            let request = deterministic;
            tokio::spawn(async move { executor.complete(&context(), request).await })
        };

        let (first, second) = tokio::join!(first, second);
        assert_eq!(
            first.expect("first task").expect("first result").id,
            "cached"
        );
        assert_eq!(
            second.expect("second task").expect("second result").id,
            "cached"
        );
        assert_eq!(
            provider.calls.load(Ordering::Relaxed),
            1,
            "concurrent identical requests share one upstream call"
        );
    }

    #[tokio::test]
    async fn cost_calculator_feeds_usage_and_metrics() {
        let provider = Arc::new(CountingSuccessProvider {
            calls: AtomicUsize::new(0),
            delay: Duration::ZERO,
        });
        let catalog = catalog_version(Arc::clone(&provider) as Arc<dyn ChatProvider>, 1);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let sink = Arc::new(MemoryUsageSink::new());
        let meter = Arc::new(MemoryMeter::new());
        let costs = crate::cost::StaticCostTable::new().with_price(
            "fast",
            crate::cost::ModelPrice::new(1_000_000, 2_000_000, 0, 0),
        );
        let executor = GatewayExecutor::new(catalog, router)
            .with_retry_policy(RetryPolicy::immediate())
            .with_cost_calculator(Arc::new(costs))
            .with_usage(Arc::clone(&sink) as Arc<dyn IUsageSink>)
            .with_meter(Arc::clone(&meter) as Arc<dyn IMeter>);

        executor.complete(&context(), request()).await.expect("ok");
        let event = sink.get("req-1").expect("usage");
        assert_eq!(event.cost_micros, 6, "2 prompt + 4 completion micros");
        assert_eq!(
            meter.counter_value("ai_gateway_cost_micros_total", &[("model", "fast")]),
            6
        );
    }

    #[tokio::test]
    async fn up_front_budget_precheck_rejects_expensive_request() {
        let provider = Arc::new(CountingSuccessProvider {
            calls: AtomicUsize::new(0),
            delay: Duration::ZERO,
        });
        let catalog = catalog_version(Arc::clone(&provider) as Arc<dyn ChatProvider>, 1);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let costs = crate::cost::StaticCostTable::new().with_price(
            "fast",
            crate::cost::ModelPrice::new(1_000_000, 1_000_000, 0, 0),
        );
        let store = quota(QuotaLimits::default().budget_micros(1));
        let executor = GatewayExecutor::new(catalog, router)
            .with_cost_calculator(Arc::new(costs))
            .with_quota(store);

        let err = executor
            .complete(&context(), request())
            .await
            .expect_err("estimated cost far exceeds the 1-micro budget");
        assert!(
            matches!(err, GatewayError::Quota(_)),
            "expected quota rejection, got {err:?}"
        );
        assert_eq!(
            provider.calls.load(Ordering::Relaxed),
            0,
            "provider must not be called when the budget pre-check rejects"
        );
    }

    #[tokio::test]
    async fn estimate_uses_default_completion_bound_and_cost() {
        let provider = Arc::new(CountingSuccessProvider {
            calls: AtomicUsize::new(0),
            delay: Duration::ZERO,
        });
        let catalog = catalog_version(Arc::clone(&provider) as Arc<dyn ChatProvider>, 1);
        let router = Arc::new(CatalogRouter::new(Arc::clone(&catalog)));
        let costs = crate::cost::StaticCostTable::new().with_price(
            "fast",
            crate::cost::ModelPrice::new(1_000_000, 1_000_000, 0, 0),
        );
        let executor = GatewayExecutor::new(catalog, router).with_cost_calculator(Arc::new(costs));

        let estimate = executor.estimate(&context(), &request());
        assert_eq!(estimate.model, "fast");
        assert_eq!(estimate.max_completion_tokens, 1024);
        assert!(estimate.prompt_tokens > 0);
        assert_eq!(
            estimate.estimated_cost_micros,
            estimate.prompt_tokens + 1024
        );
    }
}
