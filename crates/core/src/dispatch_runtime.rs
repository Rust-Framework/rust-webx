//! Per-request dispatch runtime — instance-scoped provider and handler cache.
//!
//! `Host` stores a [`DispatchRuntime`] and scopes it to each HTTP request via
//! a task-local. Macro-generated dispatch and `Mediator` read from the active
//! runtime; there is **no** process-wide fallback in 0.10 — code that runs
//! outside HTTP dispatch (in-process `Mediator::send`, tests, background work)
//! must enter a runtime explicitly with [`DispatchRuntime::run`].

use std::future::Future;
use std::sync::Arc;

use crate::route::scan::HandlerCache;

tokio::task_local! {
    static ACTIVE_RUNTIME: Option<Arc<DispatchRuntime>>;
    /// Provider of the DI scope currently active for `dispatch_provider()`.
    ///
    /// Set by [`DispatchRuntime::run_in_scope`] (hosted services) so scoped
    /// dependencies resolved by `dispatch_provider().get_owned()` bind to that
    /// scope instead of the root provider's scoped cache.
    static ACTIVE_SCOPE_PROVIDER: Option<Arc<rust_dix::ServiceProvider>>;
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

    /// Run an async block with a **fresh DI scope** active.
    ///
    /// `dispatch_provider()` resolves within the scope while the block runs, so
    /// a hosted service that does `dispatch_provider().get_owned::<DbContext>()`
    /// gets a context bound to *this* scope (transitive scoped dependencies
    /// included), not one cached on the root provider. The scope is disposed
    /// when the block finishes.
    ///
    /// Hosted services are started and stopped inside this, which is what makes
    /// their scoped dependencies per-run instead of process-wide singletons.
    pub async fn run_in_scope<F, R>(self: &Arc<Self>, f: F) -> R
    where
        F: Future<Output = R>,
    {
        let scope = self.provider.scope();
        let scoped_provider = Arc::clone(scope.scope_provider());
        let result = ACTIVE_RUNTIME
            .scope(
                Some(Arc::clone(self)),
                ACTIVE_SCOPE_PROVIDER.scope(Some(scoped_provider), f),
            )
            .await;
        scope.dispose();
        result
    }
}

/// Provider for HTTP dispatch and `#[handler(inject)]` factories.
///
/// Resolves from the active [`DispatchRuntime`]: the scope opened by
/// [`DispatchRuntime::run_in_scope`] (hosted services) first, else the runtime
/// itself. Hosts scope their runtime around every HTTP request, so endpoint
/// code always has one; a call from a thread/task with no runtime is a
/// programming error and panics.
pub fn dispatch_provider() -> Arc<rust_dix::ServiceProvider> {
    // A scope opened by `run_in_scope` (hosted service start/stop) wins: its
    // provider owns the scoped cache, so scoped dependencies stay per-scope.
    if let Ok(Some(provider)) = ACTIVE_SCOPE_PROVIDER.try_with(|p| p.clone()) {
        return provider;
    }

    if let Ok(Some(provider)) =
        ACTIVE_RUNTIME.try_with(|rt| rt.as_ref().map(|r| Arc::clone(&r.provider)))
    {
        return provider;
    }

    panic!(
        "No active DispatchRuntime. HTTP requests and hosted services run inside \
         Host::dispatch_runtime().run(); for in-process calls (Mediator::send, tests, \
         background work) enter one explicitly with `runtime.run(...)` / \
         `host.dispatch_runtime().run(...)`, or use `host.provider()` when no \
         task-local runtime is needed."
    );
}

/// Handler registry for the active dispatch context.
///
/// The registry is per host (`Host::build()` builds it once) and reached through
/// the active [`DispatchRuntime`]; there is no process-wide fallback, so calling
/// this outside a runtime panics like [`dispatch_provider`].
pub fn dispatch_handler_cache() -> Arc<HandlerCache> {
    if let Ok(Some(cache)) =
        ACTIVE_RUNTIME.try_with(|rt| rt.as_ref().map(|r| Arc::clone(&r.handler_cache)))
    {
        return cache;
    }

    panic!(
        "No active DispatchRuntime. Handler dispatch requires a host runtime: run \
         `Host::dispatch_runtime().run(...)` around the call (HTTP requests and hosted \
         services already do), or `host.dispatch_runtime()` for the host instance."
    );
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
        let svc = ServiceCollection::new().scoped::<ScopedThing>(move |_| {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            Arc::new(ScopedThing(n))
        });
        let provider = svc.build().expect("provider");
        let runtime = Arc::new(DispatchRuntime::new(
            provider,
            Arc::new(HandlerCache::build()),
        ));
        (runtime, created)
    }

    /// #20: hosted services run inside a DI scope, so `dispatch_provider()`
    /// resolves against a per-run scoped cache instead of the root provider.
    #[tokio::test]
    async fn run_in_scope_gives_each_run_its_own_scoped_cache() {
        let (runtime, created) = runtime_with_scoped_counter();

        let same_within_one_scope = runtime
            .run_in_scope(async {
                let first = dispatch_provider().get::<ScopedThing>().expect("scoped");
                let second = dispatch_provider().get::<ScopedThing>().expect("scoped");
                Arc::ptr_eq(&first, &second)
            })
            .await;
        assert!(
            same_within_one_scope,
            "scoped service must be cached per scope"
        );
        assert_eq!(created.load(Ordering::SeqCst), 1);

        runtime
            .run_in_scope(async {
                let again = dispatch_provider().get::<ScopedThing>().expect("scoped");
                assert_eq!(again.0, 1, "a new scope must not reuse the root cache");
            })
            .await;
        assert_eq!(created.load(Ordering::SeqCst), 2);
    }

    /// Outside `run_in_scope` the runtime's own provider is used (the scope
    /// override is scoped, not global).
    #[tokio::test]
    async fn scope_provider_does_not_leak_outside_run_in_scope() {
        let (runtime, _created) = runtime_with_scoped_counter();
        runtime
            .run(async {
                assert!(
                    dispatch_provider().get::<ScopedThing>().is_ok(),
                    "runtime provider still resolves the service"
                );
            })
            .await;
    }
}
