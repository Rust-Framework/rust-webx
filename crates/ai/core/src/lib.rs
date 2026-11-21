//! AI gateway domain kit for rust-webx.
//!
//! This crate is the AI-specific half of the gateway; the HTTP surface is
//! `rust-webx-ai-gateway`. It covers the canonical request/stream IR, the
//! `ChatProvider` port and OpenAI-compatible adapter, the model catalog/router
//! with `CatalogHandle` hot reload, the request executor (first-byte rules,
//! retry, quota/usage wiring), exact response cache with single-flight, resilience
//! (retry/backoff, circuit breaker, bulkhead), quota reserve/settle, usage ledger
//! sinks and rollups, cost calculation, token accounting, API-key parsing/HMAC helpers with an
//! `IApiKeyStore` port and in-memory store, and the OpenAI error envelope. The
//! executor emits structured `tracing` events for completions/failures/streams.
//! HTTP data-plane endpoints and non-OpenAI adapters are follow-up work.
//!
//! # Versioning
//!
//! `rust-webx-ai` is a workspace member with `version.workspace = true`, so it
//! always shares the `rust-webx` version number. There is no independent 0.x
//! line: a `rust-webx` 1.5.0 release ships `rust-webx-ai` 1.5.0.
//!
//! # Scope rules
//!
//! * No HTTP engine, no provider SDKs, no database code in this crate.
//! * No provider-specific fields (`openai`, `anthropic`, …) in the shared types;
//!   provider differences belong in adapters.
//! * Ports are defined here; implementations live with the application or in
//!   follow-up modules (`rust-webx-client` supplies the outbound transport).

pub mod api_key;
pub mod api_key_store;
pub mod cache;
pub mod catalog_file;
pub mod cost;
pub mod error;
pub mod executor;
pub mod ir;
pub mod openai;
pub mod provider;
pub mod quota;
pub mod resilience;
pub mod routing;
pub mod usage;

pub use api_key::{
    hash_secret, parse_presented, verify_secret, PresentedApiKey, DEFAULT_PREFIX, MIN_SECRET_LEN,
};
pub use api_key_store::{
    now_unix_secs, ApiKeyRecord, ApiKeyStatus, ApiKeyStoreError, CachedApiKeyStore, IApiKeyStore,
    MemoryApiKeyStore,
};
pub use cache::{request_cache_key, IGatewayCache, MemoryGatewayCache};
pub use catalog_file::{
    CapabilityFlags, CatalogFileError, CatalogFileWatcher, CatalogManifest, ManifestAlias,
    ManifestModel, ManifestStrategy, ManifestTarget,
};
pub use cost::{ICostCalculator, ModelPrice, NoCost, StaticCostTable};
pub use error::{OpenAiError, OpenAiErrorBody, OpenAiErrorWriter};
pub use executor::{GatewayContext, GatewayError, GatewayEstimate, GatewayExecutor, GatewayStream};
pub use ir::{
    CanonicalRequest, CanonicalResponse, ContentPart, Delta, FinishReason, Message, Role,
    SamplingParams, StreamEvent, ToolCall, ToolCallDelta, ToolDef,
};
pub use openai::OpenAiProvider;
pub use provider::{
    Alias, CanonicalStream, CatalogHandle, ChatProvider, ModelCatalog, ModelKey, ModelSpec,
    ProviderCapabilities, ProviderError, RouteStrategy, RouteTarget,
};
pub use quota::{
    IQuotaPolicy, IQuotaStore, MemoryQuotaStore, QuotaDimension, QuotaError, QuotaKey, QuotaLimits,
    Reservation, ReserveRequest, StaticQuotaPolicy,
};
pub use resilience::{
    Bulkhead, BulkheadError, BulkheadPermit, CircuitBreaker, CircuitSnapshot, CircuitState,
    RetryPolicy,
};
pub use routing::{
    Attempt, CatalogRouter, HealthPolicy, HealthRegistry, IRoutePlanner, ModelHealth, RouteError,
    RoutePlan, RouteRequest,
};
pub use usage::{
    rollup_events, HeuristicTokenizer, ITokenizer, IUsageSink, MemoryUsageSink, Usage, UsageError,
    UsageEvent, UsageRollup, UsageStatus,
};
