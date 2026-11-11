//! Host-level DI handles: the service provider and the handler registry.
//!
//! Since 1.0 rust-webx passes DI explicitly instead of relying on a task-local
//! ambient runtime:
//!
//! - [`Mediator`](crate::mediator::Mediator) holds the provider and the handler
//!   registry it was built with;
//! - [`IHttpContext`](crate::http::IHttpContext) exposes the
//!   [`DispatchRuntime`] that dispatched the request, so endpoint adapters and
//!   middleware can open scopes (`provider.scope()`) explicitly;
//! - background work injects rust-dix's built-in `Arc<dyn ScopeFactory>` and
//!   calls `ScopeFactory::create_scope()` itself.
//!
//! `Host` stores one [`DispatchRuntime`] per instance. There is no process-wide
//! or task-local fallback: `rust-dix` owns scope lifetime, resolution and
//! disposal, and rust-webx only keeps the per-host handle.

use std::sync::Arc;

use crate::route::scan::HandlerCache;

/// Provider + handler registry for one host instance.
///
/// This is a plain handle: obtaining the provider or registry requires holding
/// the runtime (or the `Host` that owns it), never an ambient task-local.
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

    /// The host's root service provider (rust-dix).
    pub fn provider(&self) -> &Arc<rust_dix::ServiceProvider> {
        &self.provider
    }

    /// The compiled handler registry of this host.
    pub fn handler_cache(&self) -> &Arc<HandlerCache> {
        &self.handler_cache
    }

    /// Open a fresh DI scope on this host's provider.
    ///
    /// Equivalent to `self.provider().scope()`; exposed for call sites that
    /// already hold the runtime (HTTP dispatch, middleware, hosted services).
    pub fn scope(&self) -> rust_dix::Scope {
        self.provider.scope()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_dix::ServiceCollection;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct ScopedThing(usize);

    fn runtime_with_scoped_counter() -> (Arc<DispatchRuntime>, Arc<AtomicUsize>) {
        let created = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&created);
        let provider = ServiceCollection::new()
            .scoped::<ScopedThing>(move |_| {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                Arc::new(ScopedThing(n))
            })
            .build()
            .expect("provider");
        (
            Arc::new(DispatchRuntime::new(
                provider,
                Arc::new(HandlerCache::build()),
            )),
            created,
        )
    }

    /// rust-dix owns strict scope validation: the runtime's handle resolves
    /// Scoped services through an explicit scope, and root resolution fails.
    #[tokio::test]
    async fn runtime_scope_is_the_explicit_di_entry_point() {
        let (runtime, created) = runtime_with_scoped_counter();

        let scope = runtime.scope();
        let first = scope.get::<ScopedThing>().expect("scoped in scope");
        let second = scope.get::<ScopedThing>().expect("scoped in scope");
        assert!(
            Arc::ptr_eq(&first, &second),
            "Scoped services are cached per scope"
        );
        assert_eq!(first.0, 0, "the scope created the service once");
        assert_eq!(created.load(Ordering::SeqCst), 1);

        let error = runtime
            .provider()
            .get::<ScopedThing>()
            .expect_err("strict scope validation rejects Scoped services from the root");
        assert!(error.to_string().contains("root provider"), "{error}");
    }
}
