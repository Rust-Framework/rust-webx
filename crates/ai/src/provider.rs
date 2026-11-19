//! Provider port, capabilities and model catalog.
//!
//! A [`ChatProvider`] is a vendor adapter. It speaks the canonical
//! [`CanonicalRequest`](crate::ir::CanonicalRequest) /
//! [`StreamEvent`](crate::ir::StreamEvent) types, so the router can treat all
//! providers uniformly. The [`ModelCatalog`] is the immutable snapshot the data
//! plane reads: which providers exist, which models they expose, and which
//! aliases route to which models.

use crate::ir::{CanonicalRequest, CanonicalResponse, StreamEvent};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

/// A stream of canonical events.
pub type CanonicalStream = BoxStream<'static, Result<StreamEvent, ProviderError>>;

/// A provider failure with just enough classification for routing.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProviderError {
    /// The request is invalid for this provider.
    #[error("invalid provider request: {0}")]
    InvalidRequest(String),
    /// The provider rejected the credential.
    #[error("provider authentication failed")]
    Unauthorized,
    /// The provider is rate limiting the caller.
    #[error("provider rate limited")]
    RateLimited {
        /// Provider hint for when to retry.
        retry_after: Option<Duration>,
    },
    /// The provider is unreachable/overloaded.
    #[error("provider unavailable: {0}")]
    Unavailable(String),
    /// A provider deadline elapsed.
    #[error("provider timed out")]
    Timeout,
    /// The provider returned a malformed/failed protocol exchange.
    #[error("provider protocol error: {0}")]
    Protocol(String),
    /// The provider does not support the requested feature.
    #[error("provider does not support: {0}")]
    Unsupported(String),
}

impl ProviderError {
    /// Whether another provider/attempt may succeed.
    ///
    /// The executor must still respect the "no retry after the first streamed
    /// token" rule.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::Unavailable(_) | Self::Timeout
        )
    }

    /// Suggested HTTP status when this error is surfaced directly.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::InvalidRequest(_) | Self::Unsupported(_) => 400,
            Self::Unauthorized => 401,
            Self::RateLimited { .. } => 429,
            Self::Unavailable(_) => 503,
            Self::Timeout => 504,
            Self::Protocol(_) => 502,
        }
    }

    /// Provider retry hint, when present.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after } => *retry_after,
            _ => None,
        }
    }
}

/// What a provider/model can do.
///
/// `supports(required)` only checks the capabilities `required` sets to `true`,
/// so `ProviderCapabilities::none()` is "no requirements".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderCapabilities {
    /// Streamed responses.
    pub streaming: bool,
    /// Tool/function calling.
    pub tools: bool,
    /// Image input.
    pub vision: bool,
    /// JSON mode / structured output.
    pub json_mode: bool,
    /// Exposed reasoning content.
    pub reasoning: bool,
    /// Embeddings endpoint.
    pub embeddings: bool,
    /// Image generation.
    pub images: bool,
    /// Audio transcription/speech.
    pub audio: bool,
}

impl ProviderCapabilities {
    /// No capabilities (also "no requirements").
    pub const fn none() -> Self {
        Self {
            streaming: false,
            tools: false,
            vision: false,
            json_mode: false,
            reasoning: false,
            embeddings: false,
            images: false,
            audio: false,
        }
    }

    /// A chat model that streams.
    pub const fn chat_stream() -> Self {
        Self {
            streaming: true,
            tools: false,
            vision: false,
            json_mode: false,
            reasoning: false,
            embeddings: false,
            images: false,
            audio: false,
        }
    }

    /// Mark streaming support.
    pub fn stream(mut self) -> Self {
        self.streaming = true;
        self
    }

    /// Mark tool-calling support.
    pub fn tools(mut self) -> Self {
        self.tools = true;
        self
    }

    /// Mark vision support.
    pub fn vision(mut self) -> Self {
        self.vision = true;
        self
    }

    /// Mark JSON-mode support.
    pub fn json_mode(mut self) -> Self {
        self.json_mode = true;
        self
    }

    /// Mark reasoning-content support.
    pub fn reasoning(mut self) -> Self {
        self.reasoning = true;
        self
    }

    /// Mark embeddings support.
    pub fn embeddings(mut self) -> Self {
        self.embeddings = true;
        self
    }

    /// Mark image-generation support.
    pub fn images(mut self) -> Self {
        self.images = true;
        self
    }

    /// Mark audio support.
    pub fn audio(mut self) -> Self {
        self.audio = true;
        self
    }

    /// Whether every capability set in `required` is present.
    pub fn supports(&self, required: Self) -> bool {
        (!required.streaming || self.streaming)
            && (!required.tools || self.tools)
            && (!required.vision || self.vision)
            && (!required.json_mode || self.json_mode)
            && (!required.reasoning || self.reasoning)
            && (!required.embeddings || self.embeddings)
            && (!required.images || self.images)
            && (!required.audio || self.audio)
    }
}

/// A vendor adapter.
#[async_trait]
pub trait ChatProvider: Send + Sync {
    /// Stable provider id, e.g. `"openai"`.
    fn id(&self) -> &str;

    /// Capabilities of this provider (the router filters on them).
    fn capabilities(&self) -> ProviderCapabilities;

    /// Complete a request without streaming.
    async fn complete(&self, request: CanonicalRequest)
        -> Result<CanonicalResponse, ProviderError>;

    /// Stream a request. Providers without streaming keep the default error.
    async fn stream(&self, _request: CanonicalRequest) -> Result<CanonicalStream, ProviderError> {
        Err(ProviderError::Unsupported(
            "this provider does not implement streaming".into(),
        ))
    }
}

/// A provider + upstream model pair.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelKey {
    /// Provider id.
    pub provider: String,
    /// Upstream model name.
    pub model: String,
}

impl ModelKey {
    /// Build a model key.
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }
}

impl std::fmt::Display for ModelKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}", self.provider, self.model)
    }
}

/// Metadata for one upstream model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    /// Provider + upstream model.
    pub key: ModelKey,
    /// Capabilities of this model.
    pub capabilities: ProviderCapabilities,
    /// Context window in tokens; `0` means unknown.
    pub context_window: u32,
    /// Whether the model may be selected.
    pub enabled: bool,
}

impl ModelSpec {
    /// An enabled model with no declared capabilities.
    pub fn new(key: ModelKey) -> Self {
        Self {
            key,
            capabilities: ProviderCapabilities::none(),
            context_window: 0,
            enabled: true,
        }
    }

    /// Declare capabilities.
    pub fn with_capabilities(mut self, capabilities: ProviderCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Declare the context window.
    pub fn with_context_window(mut self, context_window: u32) -> Self {
        self.context_window = context_window;
        self
    }

    /// Disable the model.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

/// One candidate in an alias.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteTarget {
    /// Upstream model.
    pub model: ModelKey,
    /// Relative weight for weighted routing.
    pub weight: u32,
}

impl RouteTarget {
    /// A target with weight `1`.
    pub fn new(model: ModelKey) -> Self {
        Self { model, weight: 1 }
    }

    /// Set the routing weight.
    pub fn with_weight(mut self, weight: u32) -> Self {
        self.weight = weight.max(1);
        self
    }
}

/// How an alias orders its candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteStrategy {
    /// Try candidates in catalog order (primary → backup).
    Priority,
    /// Same as priority, with failure semantics made explicit by the executor.
    Fallback,
    /// Order by weight (higher first); the executor still tries them in order.
    Weighted,
    /// Order by observed latency (lower first); unknown latency goes last.
    LeastLatency,
    /// Filter by capabilities, then catalog order.
    Capability,
}

/// A public model name mapped to ordered candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    /// Ordering strategy.
    pub strategy: RouteStrategy,
    /// Candidate models.
    pub targets: Vec<RouteTarget>,
}

impl Alias {
    /// Build an alias.
    pub fn new(strategy: RouteStrategy, targets: Vec<RouteTarget>) -> Self {
        Self { strategy, targets }
    }
}

/// A shared, atomically replaceable catalog snapshot.
///
/// The router and executor read `current()` on the hot path (no locks, no IO);
/// a config watcher builds a new [`ModelCatalog`] and calls [`reload`](Self::reload).
/// Older versions are rejected, so a slow watcher cannot roll the catalog back.
#[derive(Clone)]
pub struct CatalogHandle {
    inner: Arc<ArcSwap<ModelCatalog>>,
}

impl CatalogHandle {
    /// Wrap the initial snapshot.
    pub fn new(catalog: Arc<ModelCatalog>) -> Self {
        Self {
            inner: Arc::new(ArcSwap::from(catalog)),
        }
    }

    /// The current snapshot.
    pub fn current(&self) -> Arc<ModelCatalog> {
        self.inner.load_full()
    }

    /// Current catalog version.
    pub fn version(&self) -> u64 {
        self.inner.load().version
    }

    /// Replace the snapshot when `catalog.version` is not older.
    ///
    /// Returns `false` for a stale version (current is unchanged). Uses an
    /// atomic compare-and-swap loop so two concurrent reloads cannot roll the
    /// catalog back: a slower watcher that loaded an older version before a
    /// faster one committed will observe the new pointer and retry, then reject
    /// its own older version.
    pub fn reload(&self, catalog: Arc<ModelCatalog>) -> bool {
        loop {
            let current: Arc<ModelCatalog> = self.inner.load_full();
            if catalog.version < current.version {
                return false;
            }
            let expected_ptr = Arc::as_ptr(&current);
            let previous = self.inner.compare_and_swap(&current, Arc::clone(&catalog));
            if Arc::as_ptr(&*previous) == expected_ptr {
                // Our CAS won; `previous` is exactly the snapshot we replaced.
                return true;
            }
            // Another writer won between load and CAS. Loop and re-check the
            // version against the now-current snapshot — never blindly store.
        }
    }
}

/// Immutable catalog snapshot read by the router.
///
/// Build it once at startup / on config reload and share it behind a
/// [`CatalogHandle`] for hot reload. Do not mutate it per request.
#[derive(Clone)]
pub struct ModelCatalog {
    /// Monotonic version for hot-reload diagnostics.
    pub version: u64,
    /// Provider adapters by id.
    pub providers: HashMap<String, Arc<dyn ChatProvider>>,
    /// Model metadata by key.
    pub models: HashMap<ModelKey, ModelSpec>,
    /// Public aliases.
    pub aliases: HashMap<String, Alias>,
}

impl ModelCatalog {
    /// An empty catalog at `version`.
    pub fn new(version: u64) -> Self {
        Self {
            version,
            providers: HashMap::new(),
            models: HashMap::new(),
            aliases: HashMap::new(),
        }
    }

    /// Register a provider under its own id.
    pub fn add_provider(&mut self, provider: Arc<dyn ChatProvider>) {
        let id = provider.id().to_string();
        self.providers.insert(id, provider);
    }

    /// Register model metadata.
    pub fn add_model(&mut self, spec: ModelSpec) {
        self.models.insert(spec.key.clone(), spec);
    }

    /// Register an alias.
    pub fn add_alias(&mut self, name: impl Into<String>, alias: Alias) {
        self.aliases.insert(name.into(), alias);
    }

    /// Look up a provider by id.
    pub fn provider(&self, id: &str) -> Option<&Arc<dyn ChatProvider>> {
        self.providers.get(id)
    }

    /// Look up model metadata.
    pub fn model(&self, key: &ModelKey) -> Option<&ModelSpec> {
        self.models.get(key)
    }
}
