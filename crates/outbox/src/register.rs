//! DI registration for the outbox port and its dispatcher.
//!
//! The storage adapter owns the store: it registers the concrete [`IOutbox`]
//! (with a durable database, or the built-in
//! [`InMemoryOutbox`](crate::InMemoryOutbox)) through
//! [`add_outbox`](OutboxServiceCollectionExt::add_outbox) /
//! [`add_outbox_with`](OutboxServiceCollectionExt::add_outbox_with), and this
//! module adds the dispatcher. No schema is created here and no routes are
//! declared.

use std::sync::Arc;

use rust_dix::{IServiceResolver, ScopeFactory, ServiceCollection};
use webx_core::hosted::BackgroundServiceSpec;
use webx_core::route::ext::IServiceCollectionExt;
use webx_host::server::HostBuilder;

use crate::options::OutboxOptions;
use crate::port::IOutbox;
use crate::worker::OutboxDispatcher;

/// Register the outbox port and its dispatcher on a [`ServiceCollection`].
pub trait OutboxServiceCollectionExt: Sized {
    /// Register an already-built store as `Arc<dyn IOutbox>` and, when
    /// `options.enabled`, the [`OutboxDispatcher`] background service.
    fn add_outbox(self, store: Arc<dyn IOutbox>, options: OutboxOptions) -> Self {
        self.add_outbox_with(move |_| Arc::clone(&store), options)
    }

    /// Register a store built from the container (for stores that need a
    /// `ScopeFactory` or other services) and, when `options.enabled`, the
    /// [`OutboxDispatcher`] background service.
    fn add_outbox_with<F>(self, store: F, options: OutboxOptions) -> Self
    where
        F: Fn(&dyn IServiceResolver) -> Arc<dyn IOutbox> + Send + Sync + 'static;
}

impl OutboxServiceCollectionExt for ServiceCollection {
    fn add_outbox_with<F>(self, store: F, options: OutboxOptions) -> Self
    where
        F: Fn(&dyn IServiceResolver) -> Arc<dyn IOutbox> + Send + Sync + 'static,
    {
        let enabled = options.enabled;
        let worker_options = options.clone();
        self.try_add_singleton::<dyn IOutbox>(store)
            .add_background_service(
                BackgroundServiceSpec::with_factory(move |resolver| {
                    let services = resolve_scope_factory(resolver);
                    let provider = resolver.provider_arc().expect(
                        "rust-dix built-in ScopeFactory implies a built provider; outbox \
                         registration requires a provider built by rust-dix",
                    );
                    Arc::new(OutboxDispatcher::new(
                        services,
                        provider,
                        worker_options.clone(),
                    ))
                })
                .enabled(enabled),
            )
    }
}

/// Register the outbox on a [`HostBuilder`] (`add_outbox(...)`).
pub trait OutboxHostBuilderExt: Sized {
    /// Register an already-built store and (when enabled) the dispatcher.
    fn add_outbox(self, store: Arc<dyn IOutbox>, options: OutboxOptions) -> Self {
        self.add_outbox_with(move |_| Arc::clone(&store), options)
    }

    /// Register a store built from the container and (when enabled) the
    /// dispatcher.
    fn add_outbox_with<F>(self, store: F, options: OutboxOptions) -> Self
    where
        F: Fn(&dyn IServiceResolver) -> Arc<dyn IOutbox> + Send + Sync + 'static;
}

impl OutboxHostBuilderExt for HostBuilder {
    fn add_outbox_with<F>(self, store: F, options: OutboxOptions) -> Self
    where
        F: Fn(&dyn IServiceResolver) -> Arc<dyn IOutbox> + Send + Sync + 'static,
    {
        self.register(move |services| services.add_outbox_with(store, options))
    }
}

/// The rust-dix built-in `ScopeFactory` is always resolvable from a built
/// provider; a failure here means the provider was built by hand without it.
fn resolve_scope_factory(resolver: &dyn IServiceResolver) -> Arc<dyn ScopeFactory> {
    rust_dix::IServiceResolverExt::resolve_arc::<dyn ScopeFactory>(resolver).expect(
        "rust-dix always provides the built-in Arc<dyn ScopeFactory>; \
         outbox registration requires a provider built by rust-dix",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::InMemoryOutbox;

    fn provider(store: Arc<dyn IOutbox>) -> Arc<rust_dix::ServiceProvider> {
        ServiceCollection::new()
            .add_outbox(store, OutboxOptions::default().disabled())
            .build()
            .expect("provider")
    }

    #[test]
    fn add_outbox_registers_the_store_as_the_port() {
        let store: Arc<dyn IOutbox> = Arc::new(InMemoryOutbox::new(3));
        let provider = provider(Arc::clone(&store));
        let resolved = provider
            .get::<dyn IOutbox>()
            .expect("outbox port is registered");
        assert!(Arc::ptr_eq(&resolved, &store));
    }

    #[test]
    fn add_outbox_with_builds_the_store_lazily() {
        let provider = ServiceCollection::new()
            .add_outbox_with(
                |_| Arc::new(InMemoryOutbox::new(2)) as Arc<dyn IOutbox>,
                OutboxOptions::default().disabled(),
            )
            .build()
            .expect("provider");
        assert!(provider.get::<dyn IOutbox>().is_ok());
    }
}
