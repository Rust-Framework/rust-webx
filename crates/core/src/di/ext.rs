//! IServiceCollectionExt â€” Extension trait for rust_dicore ServiceCollection.

use crate::handler::IHostedService;
use crate::middleware::IMiddleware;
use crate::pipeline::IPipelineBehavior;
use rust_dicore::{ServiceCollection, ServiceLifetime};
use std::sync::Arc;

/// Extension methods for `rust_dicore::ServiceCollection` to enable
/// framework-level service registration patterns.
pub trait IServiceCollectionExt: Sized {
    /// Register the mediator in the DI container.
    ///
    /// The actual `IRequestHandler` and `IEventHandler` implementations
    /// are registered via `#[rust_dicore::module]` blocks (re-exported as `lrwf::module`).
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
    fn add_pipeline<T>(self) -> Self
    where
        T: IPipelineBehavior + Default + Send + Sync + 'static;

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
        // The Mediator is constructed later by Host.
        // Here we just signal that mediation is active.
        // Handler registrations happen via #[rust_dicore::module].
        MEDIATOR_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);
        self
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
        self.add(ServiceLifetime::Singleton, |_| {
            Arc::new(T::default()) as Arc<dyn IPipelineBehavior>
        })
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
