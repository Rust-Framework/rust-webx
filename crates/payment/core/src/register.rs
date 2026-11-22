//! Host/service-collection registration for the payment domain.

use std::any::type_name;
use std::sync::Arc;

use rust_dix::{IServiceResolver, IServiceResolverExt, ServiceCollection};

use crate::factory::{
    DefaultPaymentServiceFactory, IPaymentProviderBuilder, IPaymentProviderFactory,
    IPaymentServiceFactory, PaymentProviderBuilders, StaticPaymentProviderFactory,
};
use crate::payments::{PaymentRuntimeOptions, Payments};
use crate::provider::IPaymentProvider;

/// Collect every service registered under the erased trait `T`.
///
/// rust-dix 1.0 exposes `get_all_any` on `&dyn IServiceResolver`; the typed
/// `get_all` is not object-safe at that position, so registrations are
/// collected by type name and downcast once.
fn collect_any<T: ?Sized + Send + Sync + 'static>(resolver: &dyn IServiceResolver) -> Vec<Arc<T>> {
    resolver
        .get_all_any(type_name::<T>())
        .into_iter()
        .filter_map(|service| {
            service
                .downcast::<Arc<T>>()
                .ok()
                .map(|shared| Arc::clone(&*shared))
        })
        .collect()
}

/// Register payment services on a [`ServiceCollection`] (tests/manual wiring).
///
/// Adapters call these helpers from their `add_*` methods; applications normally
/// only call the adapter methods on the host builder. There is deliberately no
/// `add_payments()`: the runtime is created lazily and idempotently by the
/// first `add_*`. A custom [`IPaymentServiceFactory`] is a normal DI singleton:
/// register it with `services.singleton::<dyn IPaymentServiceFactory>(…)` or
/// `services.instance::<dyn IPaymentServiceFactory>(…)`; no payment-specific
/// override API exists.
pub trait PaymentsServiceCollectionExt: Sized {
    /// Register a prebuilt provider (tests, custom wiring).
    fn add_payment_provider(self, provider: Arc<dyn IPaymentProvider>) -> Self;

    /// Register an adapter-level provider factory (what `add_*` does).
    fn add_payment_provider_factory<T>(self, factory: Arc<T>) -> Self
    where
        T: IPaymentProviderFactory + 'static;

    /// Register a lazily-constructed adapter-level provider factory.
    fn add_payment_provider_factory_lazy<T, F>(self, factory: F) -> Self
    where
        T: IPaymentProviderFactory + 'static,
        F: Fn(&dyn IServiceResolver) -> Arc<T> + Send + Sync + 'static;

    /// Register an adapter builder for custom service factories.
    fn add_payment_provider_builder<T>(self, builder: Arc<T>) -> Self
    where
        T: IPaymentProviderBuilder + 'static;

    /// Ensure the `Payments` runtime exists (idempotent).
    ///
    /// Adapter `add_*` methods call this; applications never call it directly.
    #[doc(hidden)]
    fn ensure_payments(self) -> Self;
}

impl PaymentsServiceCollectionExt for ServiceCollection {
    fn add_payment_provider(self, provider: Arc<dyn IPaymentProvider>) -> Self {
        let static_factory = Arc::new(StaticPaymentProviderFactory::new(Arc::clone(&provider)));
        self.instance::<dyn IPaymentProvider>(provider)
            .add_payment_provider_factory(static_factory)
    }

    fn add_payment_provider_factory<T>(self, factory: Arc<T>) -> Self
    where
        T: IPaymentProviderFactory + 'static,
    {
        let erased: Arc<dyn IPaymentProviderFactory> = factory;
        self.instance::<dyn IPaymentProviderFactory>(erased)
            .ensure_payments()
    }

    fn add_payment_provider_factory_lazy<T, F>(self, factory: F) -> Self
    where
        T: IPaymentProviderFactory + 'static,
        F: Fn(&dyn IServiceResolver) -> Arc<T> + Send + Sync + 'static,
    {
        self.singleton::<dyn IPaymentProviderFactory>(move |resolver| {
            let concrete = factory(resolver);
            concrete as Arc<dyn IPaymentProviderFactory>
        })
        .ensure_payments()
    }

    fn add_payment_provider_builder<T>(self, builder: Arc<T>) -> Self
    where
        T: IPaymentProviderBuilder + 'static,
    {
        let erased: Arc<dyn IPaymentProviderBuilder> = builder;
        self.instance::<dyn IPaymentProviderBuilder>(erased)
            .ensure_payments()
    }

    fn ensure_payments(self) -> Self {
        let services = self
            .try_add_singleton::<PaymentProviderBuilders>(|resolver| {
                Arc::new(PaymentProviderBuilders::new(collect_any::<
                    dyn IPaymentProviderBuilder,
                >(resolver)))
            })
            // Native DI fallback: a user registration of
            // `dyn IPaymentServiceFactory` (before or after `add_*`) overrides
            // this via MEDI last-registration-wins semantics.
            .try_add_singleton::<dyn IPaymentServiceFactory>(|resolver| {
                Arc::new(DefaultPaymentServiceFactory::new(collect_any::<
                    dyn IPaymentProviderFactory,
                >(
                    resolver
                )))
            });
        services.try_add_singleton::<Payments>(|resolver| {
            let factory = resolver
                .resolve_arc::<dyn IPaymentServiceFactory>()
                .expect("default payment service factory is registered");
            let options = resolver
                .resolve_arc::<PaymentRuntimeOptions>()
                .map(|options| (*options).clone())
                .unwrap_or_default();
            Arc::new(Payments::new(factory, options))
        })
    }
}
