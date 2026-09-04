//! Per-request dispatch runtime — instance-scoped provider and handler cache.
//!
//! `Host` stores a [`DispatchRuntime`] and scopes it to each HTTP request via
//! a task-local. Macro-generated dispatch and `Mediator` read from the active
//! runtime when present, falling back to process-wide globals for backward
//! compatibility (in-process `Mediator::send` without HTTP context).

use std::future::Future;
use std::sync::Arc;

use crate::route::scan::{try_global_provider, HandlerCache};

tokio::task_local! {
    static ACTIVE_RUNTIME: Option<Arc<DispatchRuntime>>;
}

/// Provider + handler registry for one host instance.
#[derive(Clone)]
pub struct DispatchRuntime {
    provider: Arc<rust_dix::ServiceProvider>,
    handler_cache: Arc<HandlerCache>,
}

impl DispatchRuntime {
    pub fn new(provider: Arc<rust_dix::ServiceProvider>, handler_cache: Arc<HandlerCache>) -> Self {
        Self {
            provider,
            handler_cache,
        }
    }

    pub fn provider(&self) -> &Arc<rust_dix::ServiceProvider> {
        &self.provider
    }

    pub fn handler_cache(&self) -> &Arc<HandlerCache> {
        &self.handler_cache
    }

    /// Run an async block with this runtime active for the current task.
    pub async fn run<F, R>(&self, f: F) -> R
    where
        F: Future<Output = R>,
    {
        ACTIVE_RUNTIME.scope(Some(Arc::new(self.clone())), f).await
    }
}

/// Provider for HTTP dispatch and `#[handler(inject)]` factories.
///
/// Prefers the active [`DispatchRuntime`] (HTTP requests and hosted services started
/// by `Host`). Falls back to the deprecated process-wide shim when
/// [`set_global_provider`](crate::route::scan::set_global_provider) was called manually.
pub fn dispatch_provider() -> Arc<rust_dix::ServiceProvider> {
    if let Ok(Some(provider)) =
        ACTIVE_RUNTIME.try_with(|rt| rt.as_ref().map(|r| Arc::clone(&r.provider)))
    {
        return provider;
    }

    if let Some(provider) = try_global_provider() {
        tracing::warn!(
            "dispatch_provider() using deprecated global ServiceProvider shim; \
             prefer Host::dispatch_runtime().run() or host.provider()"
        );
        return provider;
    }

    panic!(
        "No active DispatchRuntime and no deprecated global ServiceProvider. \
         HTTP and hosted services run inside Host::dispatch_runtime().run(); \
         for tests use host.dispatch_runtime().run with an async block, or host.provider()."
    );
}

/// Handler registry for the active dispatch context.
///
/// Prefers the active [`DispatchRuntime`]; falls back to a lazy-built process-wide
/// registry (inventory is process-wide; contents are identical per host).
pub fn dispatch_handler_cache() -> Arc<HandlerCache> {
    if let Ok(Some(cache)) =
        ACTIVE_RUNTIME.try_with(|rt| rt.as_ref().map(|r| Arc::clone(&r.handler_cache)))
    {
        return cache;
    }

    #[allow(deprecated)]
    {
        HandlerCache::get_or_init()
    }
}
