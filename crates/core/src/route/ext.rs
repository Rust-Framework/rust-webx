//! IServiceCollectionExt — Extension trait for rust_dix ServiceCollection.

use crate::handler::IHostedService;
use crate::hosted::BackgroundServiceSpec;
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
    /// Also registers the **default event transport**: an
    /// `InMemoryEventPublisher` (bounded queue), its `BackgroundService` worker,
    /// and an `EventDispatcher` for broker consumers. Replace the transport with
    /// `HostBuilder::use_event_publisher(...)` or an application
    /// `IEventPublisher` registration.
    ///
    /// `IRequestHandler` and `IEventHandler` implementations are discovered
    /// from inventory; event handlers use `#[handler]` / `#[handler(inject)]`
    /// exactly like request handlers.
    fn add_mediator(self) -> Self;

    /// Signal that endpoints should be auto-discovered from `#[get]` / `#[endpoint]`
    /// annotations at compile time. The actual route table is built during
    /// `Host::build()` by scanning the inventory for `RouteEntry` items.
    ///
    /// This marker is a no-op: the inventory scan is on by default. Turn it off
    /// with `HostBuilder::use_inventory_routes(false)` or narrow it with
    /// `HostBuilder::routes_from(&[..])` when dependency-library routes should
    /// not be exposed.
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
    /// Register a startup/shutdown service through [`HostedServiceSpec`].
    ///
    /// ```ignore
    /// svc
    ///     .add_hosted_service(HostedServiceSpec::new::<DbInitService>())
    ///     .add_hosted_service(
    ///         HostedServiceSpec::with_factory(|resolver| {
    ///             Arc::new(OutboxConsumer::new(resolve(resolver)))
    ///         })
    ///         .ordered(-100)
    ///         .enabled(options.outbox.enabled),
    ///     );
    /// ```
    fn add_hosted_service(self, spec: crate::hosted::HostedServiceSpec) -> Self;

    /// Register a [`BackgroundService`](crate::hosted::BackgroundService)
    /// worker (singleton, `Default` constructor).
    ///
    /// The host starts the worker without blocking startup and cancels its
    /// [`CancellationToken`](crate::hosted::CancellationToken) on shutdown;
    /// business code only implements `execute`.
    /// [`CancellationToken`](crate::hosted::CancellationToken) on shutdown;
    /// business code only implements `execute`.
    ///
    /// ```ignore
    /// svc.add_background_service(BackgroundServiceSpec::new::<OutboxWorker>());
    /// svc.add_background_service(
    ///     BackgroundServiceSpec::with_factory(|resolver| {
    ///         Arc::new(OutboxWorker::new(resolver))
    ///     })
    ///     .enabled(options.outbox.enabled),
    /// );
    /// ```
    fn add_background_service(self, spec: BackgroundServiceSpec) -> Self;
}

impl IServiceCollectionExt for ServiceCollection {
    fn add_mediator(self) -> Self {
        // Transient (not singleton) so build-time validation does not require a
        // fully wired ServiceProvider. Each resolution shares the root provider
        // Arc; the handler registry is registered by `Host::build` as an
        // instance (or rebuilt when the mediator is used without a host).
        MEDIATOR_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);

        // Default event transport: a bounded in-memory queue drained by a
        // background worker. The publisher and dispatcher use `try_add`, so an
        // application registration wins. A request-only application (no
        // `#[handler]` event registration) gets the publisher that fails closed
        // but no idle worker/queue.
        let local_handlers = Arc::new(crate::mediator::EventHandlerCache::build());
        let has_local_events = !local_handlers.is_empty();
        let capacity = if has_local_events {
            crate::mediator::DEFAULT_EVENT_QUEUE_CAPACITY
        } else {
            1
        };
        let (sender, receiver) =
            tokio::sync::mpsc::channel::<crate::mediator::EventMessage>(capacity);

        let mediator_registration = self
            .try_add_singleton::<crate::mediator::EventMetrics>(|_| {
                Arc::new(crate::mediator::EventMetrics::new())
            })
            .try_add_singleton::<crate::mediator::EventDispatcher>(|resolver| {
                let provider = resolver
                    .provider_arc()
                    .expect("ServiceProvider not available from DI resolver");
                Arc::new(crate::mediator::EventDispatcher::new(provider))
            })
            .try_add_singleton::<dyn crate::mediator::IEventPublisher>(move |_| {
                Arc::new(crate::mediator::InMemoryEventPublisher::from_parts(
                    sender.clone(),
                    Arc::clone(&local_handlers),
                )) as Arc<dyn crate::mediator::IEventPublisher>
            });

        let mediator_registration = if has_local_events {
            let receiver_slot = Arc::new(std::sync::Mutex::new(Some(receiver)));
            mediator_registration.add_background_service(BackgroundServiceSpec::with_factory(
                move |resolver| {
                    let provider = resolver
                        .provider_arc()
                        .expect("ServiceProvider not available from DI resolver");
                    let receiver = receiver_slot
                        .lock()
                        .expect("event worker receiver lock poisoned")
                        .take()
                        .expect("the in-memory event worker is registered twice");
                    Arc::new(crate::mediator::InMemoryEventWorker::new(
                        receiver, provider,
                    ))
                },
            ))
        } else {
            // The queue has no producer: the publisher rejects every event
            // before touching it, so there is nothing to drain.
            drop(receiver);
            mediator_registration
        };

        mediator_registration.transient::<Mediator>(|resolver| {
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
        // Per-host marker: `Host::build` decides the direct dispatch path from
        // this DI registration, so a behavior registered by another host (or a
        // test) can never change this host's path. The process-global counter
        // below is diagnostic-only and retained for `typed_pipeline_count()`.
        TYPED_BEHAVIOR_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Singleton per (Req, Rsp) pair: the factory (and therefore constructor
        // injection) runs once per provider; behaviors are stateless wrappers.
        self.singleton_value(TypedPipelineRegistered)
            .add(ServiceLifetime::Singleton, factory)
    }

    fn add_hosted_service(self, spec: crate::hosted::HostedServiceSpec) -> Self {
        let Some(factory) = spec.into_factory() else {
            return self;
        };
        self.singleton::<dyn IHostedService>(move |resolver| factory(resolver))
    }

    fn add_background_service(self, spec: BackgroundServiceSpec) -> Self {
        if !spec.enabled {
            return self;
        }
        let factory = spec.factory;
        self.add(
            ServiceLifetime::Singleton,
            move |resolver: &dyn IServiceResolver| -> Arc<dyn IHostedService> { factory(resolver) },
        )
    }
}

// Global flags so HostBuilder knows what to scan at build time.
use std::sync::atomic::{AtomicBool, AtomicUsize};
static ENDPOINT_SCAN: AtomicBool = AtomicBool::new(false);
static MEDIATOR_ACTIVE: AtomicBool = AtomicBool::new(false);
static TYPED_BEHAVIOR_COUNT: AtomicUsize = AtomicUsize::new(0);

/// DI marker registered by [`IServiceCollectionExt::add_typed_pipeline_with`].
///
/// `Host::build` decides between the direct HTTP dispatch path and the
/// mediated path from this marker, so the decision is **per host**. Before
/// 2.2 the decision used a process-global counter, which meant any other host
/// (or a test) registering a typed behavior silently downgraded every host in
/// the same process.
#[derive(Debug, Clone, Copy, Default)]
pub struct TypedPipelineRegistered;

/// Number of typed pipeline behaviors registered in this process.
///
/// Diagnostic only: dispatch-path selection is per host and driven by the
/// [`TypedPipelineRegistered`] DI marker. This counter keeps its historical
/// process-wide meaning for logs and compatibility.
pub fn typed_pipeline_count() -> usize {
    TYPED_BEHAVIOR_COUNT.load(std::sync::atomic::Ordering::Relaxed)
}

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
