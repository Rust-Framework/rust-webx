//! Named-client registry and the `IHttpClientFactory` port.

use crate::client::IHttpClient;
use crate::error::HttpClientError;
use std::collections::HashMap;
use std::sync::Arc;

/// Resolve a client by name (`"openai"`, `"anthropic"`, …).
///
/// Handlers depend on this port rather than on concrete clients, so tests can
/// register a fake without opening sockets.
pub trait IHttpClientFactory: Send + Sync {
    /// Return the client registered under `name`.
    fn create(&self, name: &str) -> Result<Arc<dyn IHttpClient>, HttpClientError>;
}

/// An immutable set of named clients, built once at startup.
///
/// Immutability is deliberate: the gateway hot path must not take a lock to
/// find a provider client. A future hot-reload story replaces the whole
/// registry behind an `ArcSwap`, not a per-request `RwLock`.
#[derive(Default)]
pub struct HttpClientRegistry {
    clients: HashMap<String, Arc<dyn IHttpClient>>,
}

impl HttpClientRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) the client named `name`.
    ///
    /// The registry key is the caller-facing name; the client's own `name()`
    /// does not have to match, but keeping them equal makes logs readable.
    pub fn insert(
        &mut self,
        name: impl Into<String>,
        client: Arc<dyn IHttpClient>,
    ) -> Option<Arc<dyn IHttpClient>> {
        self.clients.insert(name.into(), client)
    }

    /// Builder form of [`insert`](Self::insert).
    pub fn with(mut self, name: impl Into<String>, client: Arc<dyn IHttpClient>) -> Self {
        self.insert(name, client);
        self
    }

    /// Registered names, in unspecified order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.clients.keys().map(String::as_str)
    }

    /// Number of registered clients.
    pub fn len(&self) -> usize {
        self.clients.len()
    }

    /// Whether no client is registered.
    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }
}

impl IHttpClientFactory for HttpClientRegistry {
    fn create(&self, name: &str) -> Result<Arc<dyn IHttpClient>, HttpClientError> {
        self.clients
            .get(name)
            .cloned()
            .ok_or_else(|| HttpClientError::UnknownClient(name.to_string()))
    }
}
