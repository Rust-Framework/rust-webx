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

    /// Register a pipeline behavior in the DI container as a singleton.
    ///
    /// Requires `Default`, so it is only usable for behaviors without
    /// dependencies. Use [`IServiceCollectionExt::add_pipeline_with`] when the
    /// behavior needs anything from the container.
    fn add_pipeline<T>(self) -> Self
    where
        T: IPipelineBehavior + Default + Send + Sync + 'static;

    /// Register a pipeline behavior built from a factory as a singleton.
    ///
    /// Unlike [`IServiceCollectionExt::add_pipeline`], the factory receives the
    /// DI resolver, so the behavior can take constructor dependencies — exactly
    /// what the MediatR-style "constructor injection" for pipeline behaviors
    /// means here. The factory runs once (singleton), on first resolution /
    /// build-time validation; the resolver is **not** available inside
    /// [`IPipelineBehavior::handle`].
    ///
    /// The factory is also executed during `build()` (cycle and captive-dependency
    /// validation), so it must resolve dependencies through the resolver it is
    /// given — `resolver.provider_arc()` is `None` there. `&dyn IServiceResolver`
    /// is object-safe, so `get`/`get_owned` (which require a sized resolver) are
    /// unavailable; resolve type-erased instead:
    ///
    /// ```ignore
    /// use std::sync::Arc;
    /// use webx::{IServiceCollectionExt, IPipelineBehavior};
    ///
    /// svc.add_pipeline_with(|resolver| {
    ///     let clock: Arc<Clock> = resolver
    ///         .get_any(std::any::type_name::<Clock>())
    ///         .and_then(|service| service.downcast::<Arc<Clock>>().ok())
    ///         .map(|service| Arc::clone(&*service))
    ///         .expect("register Clock before the pipeline");
    ///     Arc::new(TimingBehavior { clock }) as Arc<dyn IPipelineBehavior>
    /// })
    /// ```
    ///
    /// Capturing the dependency when registering is equally valid (and shorter)
    /// when the value already exists outside the container:
    ///
    /// ```ignore
    /// let clock = Arc::clone(&clock);
    /// svc.add_pipeline_with(move |_| Arc::new(TimingBehavior { clock }))
    /// ```
    fn add_pipeline_with<F>(self, factory: F) -> Self
    where
        F: Fn(&dyn IServiceResolver) -> Arc<dyn IPipelineBehavior> + Send + Sync + 'static;

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
}

impl IServiceCollectionExt for ServiceCollection {
    fn add_mediator(self) -> Self {
        // Transient (not singleton) so build-time validation does not require a
        // fully wired ServiceProvider. Each resolution shares the root provider Arc.
        MEDIATOR_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);
        self.transient::<Mediator>(|resolver| {
            let provider = resolver
                .provider_arc()
                .expect("ServiceProvider not available from DI resolver");
            Arc::new(Mediator::new(provider))
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

    fn add_pipeline<T>(self) -> Self
    where
        T: IPipelineBehavior + Default + Send + Sync + 'static,
    {
        self.add_pipeline_with(|_| Arc::new(T::default()) as Arc<dyn IPipelineBehavior>)
    }

    fn add_pipeline_with<F>(self, factory: F) -> Self
    where
        F: Fn(&dyn IServiceResolver) -> Arc<dyn IPipelineBehavior> + Send + Sync + 'static,
    {
        // Singleton: the factory (and therefore constructor injection) runs once
        // per provider; behaviors are stateless wrappers over the request chain.
        self.add(ServiceLifetime::Singleton, factory)
    }
    fn add_hosted_service<T>(self) -> Self
    where
        T: IHostedService + Default + Send + Sync + 'static,
    {
        self.singleton::<dyn IHostedService>(|_| Arc::new(T::default()))
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
    use crate::pipeline::{BoxedNextFn, BoxedPipelineFuture};
    use std::any::Any;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Behavior whose dependency (a counter) comes from the DI container — the
    /// reason `add_pipeline_with` exists.
    struct TickingBehavior {
        ticks: Arc<AtomicU32>,
    }

    #[async_trait::async_trait]
    impl IPipelineBehavior for TickingBehavior {
        async fn handle(
            &self,
            req: Box<dyn Any + Send>,
            next: BoxedNextFn,
        ) -> crate::error::Result<Box<dyn Any + Send>> {
            self.ticks.fetch_add(1, Ordering::SeqCst);
            next(req).await
        }
    }

    /// A terminal that plays the handler's role.
    fn terminal() -> BoxedNextFn {
        Box::new(|_req: Box<dyn Any + Send>| -> BoxedPipelineFuture {
            Box::pin(async { Ok(Box::new("handler-response") as Box<dyn Any + Send>) })
        })
    }

    /// Resolve a registered singleton from the type-erased resolver the
    /// `add_pipeline_with` factory receives.
    ///
    /// `&dyn IServiceResolver` is object-safe, so `get`/`get_owned` (which
    /// require `Self: Sized`) are unavailable there; `get_any` + downcast is the
    /// portable path (it also works during build-time validation).
    fn resolve<T: Send + Sync + 'static>(resolver: &dyn IServiceResolver) -> Arc<T> {
        resolver
            .get_any(std::any::type_name::<T>())
            .and_then(|service| service.downcast::<Arc<T>>().ok())
            .map(|service| Arc::clone(&*service))
            .expect("the dependency must be registered before the pipeline")
    }

    #[tokio::test]
    async fn add_pipeline_with_constructor_injects_from_the_resolver() {
        let ticks = Arc::new(AtomicU32::new(0));

        let provider = ServiceCollection::new()
            .instance(Arc::clone(&ticks))
            .add_pipeline_with(|resolver| {
                // The factory receives the DI resolver, so the behavior can take
                // constructor dependencies.
                let ticks: Arc<AtomicU32> = resolve::<AtomicU32>(resolver);
                Arc::new(TickingBehavior { ticks }) as Arc<dyn IPipelineBehavior>
            })
            .build()
            .expect("provider must build");

        let behavior = provider
            .get::<dyn IPipelineBehavior>()
            .expect("the behavior must be registered as a singleton");
        let response = behavior
            .handle(Box::new("request"), terminal())
            .await
            .expect("behavior must pass through to the next step");
        assert_eq!(
            *response.downcast::<&str>().expect("handler response"),
            "handler-response"
        );
        assert_eq!(
            ticks.load(Ordering::SeqCst),
            1,
            "the behavior must use the dependency injected through the resolver"
        );

        // Singleton: resolving again reuses the instance, the factory ran once.
        provider
            .get::<dyn IPipelineBehavior>()
            .expect("the behavior must be registered as a singleton")
            .handle(Box::new("request"), terminal())
            .await
            .expect("behavior must pass through to the next step");
        assert_eq!(ticks.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn add_pipeline_keeps_registering_default_behaviors() {
        #[derive(Default)]
        struct NoopBehavior;

        #[async_trait::async_trait]
        impl IPipelineBehavior for NoopBehavior {
            async fn handle(
                &self,
                req: Box<dyn Any + Send>,
                next: BoxedNextFn,
            ) -> crate::error::Result<Box<dyn Any + Send>> {
                next(req).await
            }
        }

        let provider = ServiceCollection::new()
            .add_pipeline::<NoopBehavior>()
            .build()
            .expect("provider must build");

        let response = provider
            .get::<dyn IPipelineBehavior>()
            .expect("`add_pipeline` must still register a zero-dependency behavior")
            .handle(Box::new("request"), terminal())
            .await
            .expect("behavior must pass through to the next step");
        assert_eq!(
            *response.downcast::<&str>().expect("handler response"),
            "handler-response"
        );
    }
}
