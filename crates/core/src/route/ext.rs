//! IServiceCollectionExt — Extension trait for rust_dix ServiceCollection.

use crate::handler::IHostedService;
use crate::mediator::Mediator;
use crate::middleware::IMiddleware;
use crate::pipeline::IPipelineBehavior;
use rust_dix::{IServiceResolver, ServiceCollection, ServiceLifetime};
use std::sync::Arc;

/// Extension methods for `rust_dix::ServiceCollection` to enable
/// framework-level service registration patterns.
pub trait IServiceCollectionExt: Sized {
    /// Register the mediator in the DI container.
    ///
    /// The actual `IRequestHandler` and `IEventHandler` implementations
    /// are registered via `#[rust_dix::module]` blocks (re-exported as `webx::module`).
    fn add_mediator(self) -> Self;

    /// Signal that endpoints should be auto-discovered from `#[get]` / `#[endpoint]`
    /// annotations at compile time.  The actual route table is built during
    /// `Host::build()` by scanning the inventory for `RouteEntry` items.
    fn add_request_endpoints(self) -> Self;

    /// Register controller types scoped per request.
    fn add_controllers(self) -> Self;

    /// Register a middleware type in the DI container as a singleton.
    fn add_middleware<T>(self) -> Self
    where
        T: IMiddleware + Default + Send + Sync + 'static;

    /// Register a **typed** pipeline behavior in the DI container as a singleton.
    ///
    /// `B` declares the request/response types by implementing
    /// [`IPipelineBehavior<Req, Rsp>`]; register those same types here:
    ///
    /// ```ignore
    /// #[derive(Default)]
    /// struct AuditBehavior;
    ///
    /// #[async_trait::async_trait]
    /// impl IPipelineBehavior<CreateOrderRequest, OrderView> for AuditBehavior {
    ///     async fn handle(&self, req: CreateOrderRequest, next: Next<CreateOrderRequest, OrderView>)
    ///         -> ActionResult<OrderView> { next.run(req).await }
    /// }
    ///
    /// svc.add_typed_pipeline::<AuditBehavior, CreateOrderRequest, OrderView>();
    /// ```
    ///
    /// The behavior only runs for that request/response pair. Short-circuiting is
    /// type-checked: return `Rsp` without calling
    /// [`Next::run`](crate::pipeline::Next::run).
    fn add_typed_pipeline<B, Req, Rsp>(self) -> Self
    where
        B: IPipelineBehavior<Req, Rsp> + Default + 'static,
        Req: Send + 'static,
        Rsp: Send + 'static;

    /// Register a typed pipeline behavior built from a factory (singleton).
    ///
    /// The factory receives the DI resolver at construction time, so the
    /// behavior can take constructor dependencies. It runs once per provider;
    /// the resolver is **not** available inside `handle`.
    fn add_typed_pipeline_with<F, Req, Rsp>(self, factory: F) -> Self
    where
        F: Fn(&dyn IServiceResolver) -> Arc<dyn IPipelineBehavior<Req, Rsp>>
            + Send
            + Sync
            + 'static,
        Req: Send + 'static,
        Rsp: Send + 'static;

    /// Register a hosted service in the DI container as a singleton.
    ///
    /// Hosted services are started when the host starts (before accepting
    /// requests) and stopped during graceful shutdown.
    ///
    /// Analogous to ASP.NET Core's `AddHostedService<T>()`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// Host::builder()
    ///     .register(|svc| {
    ///         svc.add_hosted_service::<DbInitService>()
    ///     })
    ///     .build()
    ///     .run()
    ///     .await
    /// ```
    fn add_hosted_service<T>(self) -> Self
    where
        T: IHostedService + Default + Send + Sync + 'static;

    /// Register a hosted service built by `factory` (singleton).
    ///
    /// Unlike [`add_hosted_service`](IServiceCollectionExt::add_hosted_service)
    /// this does **not** require `Default`: the factory receives the DI resolver
    /// when the service is constructed, so configuration and constructor
    /// dependencies are injected normally.
    ///
    /// Analogous to ASP.NET Core's
    /// `AddHostedService(sp => new T(sp.GetRequiredService<...>()))`.
    ///
    /// ```ignore
    /// svc
    ///     .add_options::<OutboxOptions>("Outbox")
    ///     .add_hosted_service_with(|resolver| {
    ///         let options = resolve::<OutboxOptions>(resolver);
    ///         Arc::new(OutboxConsumer::new(options))
    ///     })
    /// ```
    fn add_hosted_service_with<T, F>(self, factory: F) -> Self
    where
        T: IHostedService + 'static,
        F: Fn(&dyn IServiceResolver) -> Arc<T> + Send + Sync + 'static;

    /// Register a hosted service with an explicit start/stop order.
    ///
    /// Lower `order` starts first and stops last; equal orders keep the
    /// registration order. See [`IHostedService::order`].
    fn add_hosted_service_ordered<T>(self, order: i32) -> Self
    where
        T: IHostedService + Default + Send + Sync + 'static;

    /// Register a factory-built hosted service with an explicit start/stop order.
    fn add_hosted_service_with_ordered<T, F>(self, order: i32, factory: F) -> Self
    where
        T: IHostedService + 'static,
        F: Fn(&dyn IServiceResolver) -> Arc<T> + Send + Sync + 'static;

    /// Register a hosted service only when `enabled` is true.
    ///
    /// Conditions belong to configuration (`if options.Outbox.Enabled`), so a
    /// deployment can switch a migration/consumer off without a code change:
    ///
    /// ```ignore
    /// svc.add_hosted_service_if::<SeedService>(options.seed.enabled)
    /// ```
    fn add_hosted_service_if<T>(self, enabled: bool) -> Self
    where
        T: IHostedService + Default + Send + Sync + 'static;

    /// Register a factory-built hosted service only when `enabled` is true.
    fn add_hosted_service_with_if<T, F>(self, enabled: bool, factory: F) -> Self
    where
        T: IHostedService + 'static,
        F: Fn(&dyn IServiceResolver) -> Arc<T> + Send + Sync + 'static;

    /// Register a [`BackgroundService`](crate::hosted::BackgroundService)
    /// worker (singleton, `Default` constructor).
    ///
    /// The host starts the worker without blocking startup and cancels its
    /// [`CancellationToken`](crate::hosted::CancellationToken) on shutdown;
    /// business code only implements `execute`.
    fn add_background_service<B>(self) -> Self
    where
        B: crate::hosted::BackgroundService + Default;

    /// Register a factory-built [`BackgroundService`](crate::hosted::BackgroundService)
    /// worker (singleton).
    ///
    /// The factory receives the DI resolver at construction time, so the worker
    /// can take configuration and `Arc<dyn ScopeFactory>` for its per-tick
    /// scopes — the pattern that used to require a hand-rolled
    /// `start`/`stop`/`JoinHandle` hosted service.
    fn add_background_service_with<B, F>(self, factory: F) -> Self
    where
        B: crate::hosted::BackgroundService,
        F: Fn(&dyn IServiceResolver) -> Arc<B> + Send + Sync + 'static;

    /// Register a factory-built background worker only when `enabled` is true.
    fn add_background_service_with_if<B, F>(self, enabled: bool, factory: F) -> Self
    where
        B: crate::hosted::BackgroundService,
        F: Fn(&dyn IServiceResolver) -> Arc<B> + Send + Sync + 'static;
}

impl IServiceCollectionExt for ServiceCollection {
    fn add_mediator(self) -> Self {
        // Transient (not singleton) so build-time validation does not require a
        // fully wired ServiceProvider. Each resolution shares the root provider
        // Arc; the handler registry is registered by `Host::build` as an
        // instance (or rebuilt when the mediator is used without a host).
        MEDIATOR_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);
        self.transient::<Mediator>(|resolver| {
            let provider = resolver
                .provider_arc()
                .expect("ServiceProvider not available from DI resolver");
            let handler_cache = resolver
                .get_any(std::any::type_name::<Arc<crate::route::scan::HandlerCache>>())
                .and_then(|service| {
                    service
                        .downcast::<Arc<crate::route::scan::HandlerCache>>()
                        .ok()
                })
                .map(|service| Arc::clone(&*service))
                .unwrap_or_else(|| Arc::new(crate::route::scan::HandlerCache::build()));
            Arc::new(Mediator::new(provider, handler_cache))
        })
    }

    fn add_request_endpoints(self) -> Self {
        // Routes are collected from inventory at Host::build() time.
        // This call just signals that auto-scanning is desired.
        ENDPOINT_SCAN.store(true, std::sync::atomic::Ordering::SeqCst);
        self
    }

    fn add_controllers(self) -> Self {
        self
    }

    fn add_middleware<T>(self) -> Self
    where
        T: IMiddleware + Default + Send + Sync + 'static,
    {
        self.singleton::<dyn IMiddleware>(|_| Arc::new(T::default()))
    }

    fn add_typed_pipeline<B, Req, Rsp>(self) -> Self
    where
        B: IPipelineBehavior<Req, Rsp> + Default + 'static,
        Req: Send + 'static,
        Rsp: Send + 'static,
    {
        self.add_typed_pipeline_with::<_, Req, Rsp>(|_| {
            Arc::new(B::default()) as Arc<dyn IPipelineBehavior<Req, Rsp>>
        })
    }

    fn add_typed_pipeline_with<F, Req, Rsp>(self, factory: F) -> Self
    where
        F: Fn(&dyn IServiceResolver) -> Arc<dyn IPipelineBehavior<Req, Rsp>>
            + Send
            + Sync
            + 'static,
        Req: Send + 'static,
        Rsp: Send + 'static,
    {
        // Singleton per (Req, Rsp) pair: the factory (and therefore constructor
        // injection) runs once per provider; behaviors are stateless wrappers.
        self.add(ServiceLifetime::Singleton, factory)
    }

    fn add_hosted_service<T>(self) -> Self
    where
        T: IHostedService + Default + Send + Sync + 'static,
    {
        self.singleton::<dyn IHostedService>(|_| Arc::new(T::default()))
    }

    fn add_hosted_service_with<T, F>(self, factory: F) -> Self
    where
        T: IHostedService + 'static,
        F: Fn(&dyn IServiceResolver) -> Arc<T> + Send + Sync + 'static,
    {
        self.add(
            ServiceLifetime::Singleton,
            move |resolver: &dyn IServiceResolver| -> Arc<dyn IHostedService> { factory(resolver) },
        )
    }

    fn add_hosted_service_ordered<T>(self, order: i32) -> Self
    where
        T: IHostedService + Default + Send + Sync + 'static,
    {
        self.add_hosted_service_with_ordered(order, |_| Arc::new(T::default()))
    }

    fn add_hosted_service_with_ordered<T, F>(self, order: i32, factory: F) -> Self
    where
        T: IHostedService + 'static,
        F: Fn(&dyn IServiceResolver) -> Arc<T> + Send + Sync + 'static,
    {
        self.add(
            ServiceLifetime::Singleton,
            move |resolver: &dyn IServiceResolver| -> Arc<dyn IHostedService> {
                Arc::new(crate::hosted::OrderedHostedService::new(
                    order,
                    factory(resolver),
                ))
            },
        )
    }

    fn add_hosted_service_if<T>(self, enabled: bool) -> Self
    where
        T: IHostedService + Default + Send + Sync + 'static,
    {
        if enabled {
            self.add_hosted_service::<T>()
        } else {
            self
        }
    }

    fn add_hosted_service_with_if<T, F>(self, enabled: bool, factory: F) -> Self
    where
        T: IHostedService + 'static,
        F: Fn(&dyn IServiceResolver) -> Arc<T> + Send + Sync + 'static,
    {
        if enabled {
            self.add_hosted_service_with(factory)
        } else {
            self
        }
    }

    fn add_background_service<B>(self) -> Self
    where
        B: crate::hosted::BackgroundService + Default,
    {
        self.add_background_service_with(|_| Arc::new(B::default()))
    }

    fn add_background_service_with<B, F>(self, factory: F) -> Self
    where
        B: crate::hosted::BackgroundService,
        F: Fn(&dyn IServiceResolver) -> Arc<B> + Send + Sync + 'static,
    {
        self.add(
            ServiceLifetime::Singleton,
            move |resolver: &dyn IServiceResolver| -> Arc<dyn IHostedService> {
                Arc::new(crate::hosted::BackgroundServiceHost::new(factory(resolver)))
            },
        )
    }

    fn add_background_service_with_if<B, F>(self, enabled: bool, factory: F) -> Self
    where
        B: crate::hosted::BackgroundService,
        F: Fn(&dyn IServiceResolver) -> Arc<B> + Send + Sync + 'static,
    {
        if enabled {
            self.add_background_service_with(factory)
        } else {
            self
        }
    }
}

// Global flags so HostBuilder knows what to scan at build time.
use std::sync::atomic::AtomicBool;
static ENDPOINT_SCAN: AtomicBool = AtomicBool::new(false);
static MEDIATOR_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn should_scan_endpoints() -> bool {
    ENDPOINT_SCAN.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn is_mediator_active() -> bool {
    MEDIATOR_ACTIVE.load(std::sync::atomic::Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ActionResult;
    use crate::pipeline::{IPipelineBehavior, Next};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Resolve a registered singleton from the type-erased resolver the
    /// registration factories receive.
    fn resolve<T: Send + Sync + 'static>(resolver: &dyn IServiceResolver) -> Arc<T> {
        resolver
            .get_any(std::any::type_name::<T>())
            .and_then(|service| service.downcast::<Arc<T>>().ok())
            .map(|service| Arc::clone(&*service))
            .expect("the dependency must be registered before the pipeline")
    }

    // ----- typed behaviors (0.10) -----

    #[derive(Default)]
    struct Uppercase;

    #[async_trait::async_trait]
    impl IPipelineBehavior<u32, String> for Uppercase {
        async fn handle(&self, req: u32, next: Next<u32, String>) -> ActionResult<String> {
            let inner = next.run(req + 1).await?;
            Ok(inner.to_uppercase())
        }
    }

    fn string_terminal() -> Next<u32, String> {
        Next::new(|n: u32| async move { Ok::<String, crate::error::Error>(format!("n={n}")) })
    }

    #[tokio::test]
    async fn typed_behavior_registers_only_for_its_request_response_pair() {
        let provider = ServiceCollection::new()
            .add_typed_pipeline::<Uppercase, u32, String>()
            .build()
            .expect("provider must build");

        let behaviors = provider.get_all::<dyn IPipelineBehavior<u32, String>>();
        assert_eq!(behaviors.len(), 1);
        let out = behaviors[0]
            .handle(7, string_terminal())
            .await
            .expect("typed behavior must run");
        assert_eq!(out, "N=8");

        assert!(
            provider
                .get_all::<dyn IPipelineBehavior<u64, String>>()
                .is_empty(),
            "a typed behavior must only attach to its own request/response pair"
        );
    }

    struct TickBehavior {
        ticks: Arc<AtomicU32>,
    }

    #[async_trait::async_trait]
    impl IPipelineBehavior<u32, u32> for TickBehavior {
        async fn handle(&self, req: u32, next: Next<u32, u32>) -> ActionResult<u32> {
            self.ticks.fetch_add(1, Ordering::SeqCst);
            next.run(req).await
        }
    }

    #[tokio::test]
    async fn typed_add_pipeline_with_constructor_injects_from_the_resolver() {
        let ticks = Arc::new(AtomicU32::new(0));

        let provider = ServiceCollection::new()
            .instance(Arc::clone(&ticks))
            .add_typed_pipeline_with(|resolver| {
                let ticks: Arc<AtomicU32> = resolve::<AtomicU32>(resolver);
                Arc::new(TickBehavior { ticks }) as Arc<dyn IPipelineBehavior<u32, u32>>
            })
            .build()
            .expect("provider must build");

        let behavior = &provider.get_all::<dyn IPipelineBehavior<u32, u32>>()[0];
        let out = behavior
            .handle(5, Next::new(|n: u32| async move { Ok(n * 2) }))
            .await
            .expect("typed behavior must run");
        assert_eq!(out, 10);
        assert_eq!(ticks.load(Ordering::SeqCst), 1);
    }

    struct ShortCircuit;

    #[async_trait::async_trait]
    impl IPipelineBehavior<u32, String> for ShortCircuit {
        async fn handle(&self, _req: u32, _next: Next<u32, String>) -> ActionResult<String> {
            Ok("short".to_string())
        }
    }

    #[tokio::test]
    async fn try_add_keeps_the_existing_registration() {
        let provider = ServiceCollection::new()
            .instance(Arc::new(1u32))
            .try_add(|_| Arc::new(2u32))
            .build()
            .expect("provider must build");

        let value = provider.get::<u32>().expect("registered");
        assert_eq!(
            *value, 1,
            "try_add must not replace an earlier registration (MEDI TryAdd); rust-dix 0.8+ resolves the last registration otherwise"
        );
    }

    #[tokio::test]
    async fn typed_behavior_may_short_circuit_with_its_response_type() {
        let out = ShortCircuit
            .handle(1, string_terminal())
            .await
            .expect("short-circuit must return the typed response");
        assert_eq!(out, "short");
    }
}
