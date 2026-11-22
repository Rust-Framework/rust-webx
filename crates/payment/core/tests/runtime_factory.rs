//! Runtime registration without `add_payments()`: custom service factory,
//! provider factory routing and builder registry.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use webx::{AppMode, Host};
use webx_pay::{
    CreatePayment, IPaymentProvider, IPaymentProviderBuilder, IPaymentProviderFactory,
    IPaymentServiceFactory, IPaymentWebhookVerifier, PaymentAccount, PaymentError, PaymentEvent,
    PaymentIntent, PaymentOption, PaymentProviderBuilders, PaymentProviderConfig,
    PaymentRuntimeOptions, PaymentScope, Payments, PaymentsServiceCollectionExt,
    ProviderCapabilities, ProviderId, Refund, RefundCommand, WebhookCapability, WebhookPayload,
};

struct FakeVerifier;

#[async_trait]
impl IPaymentWebhookVerifier for FakeVerifier {
    fn provider(&self) -> ProviderId {
        ProviderId::Stripe
    }

    async fn verify(
        &self,
        _scope: &PaymentScope,
        _payload: &WebhookPayload,
    ) -> Result<PaymentEvent, PaymentError> {
        Err(PaymentError::internal("not used"))
    }

    fn ack(&self) -> webx_core::action::Response {
        webx_pay::text_success_ack()
    }
}

struct FakeProvider {
    verifier: FakeVerifier,
}

#[async_trait]
impl IPaymentProvider for FakeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Stripe
    }

    fn accounts(&self) -> &[PaymentAccount] {
        &[]
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            currencies: webx_pay::CurrencyScope::Only(&["CNY"]),
            ..ProviderCapabilities::basic(WebhookCapability::HmacSha256)
        }
    }

    fn payment_options(&self) -> Vec<PaymentOption> {
        vec![PaymentOption::new(
            ProviderId::Stripe,
            webx_pay::AccountId::default_account(),
            webx_pay::PaymentMethod::Card,
            [webx_pay::PaymentFlowKind::ClientSdk],
        )]
    }

    fn validate(&self) -> Result<(), PaymentError> {
        Ok(())
    }

    async fn create_payment(
        &self,
        _scope: &PaymentScope,
        _request: CreatePayment,
    ) -> Result<PaymentIntent, PaymentError> {
        Err(PaymentError::internal("not used"))
    }

    async fn get_payment(
        &self,
        _scope: &PaymentScope,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        Err(PaymentError::not_found(ProviderId::Stripe, order_id))
    }

    async fn refund(
        &self,
        _scope: &PaymentScope,
        _request: RefundCommand,
    ) -> Result<Refund, PaymentError> {
        Err(PaymentError::unsupported(ProviderId::Stripe, "refund"))
    }

    fn webhook_verifier(&self) -> &dyn IPaymentWebhookVerifier {
        &self.verifier
    }
}

struct FakeServiceFactory {
    calls: AtomicUsize,
}

#[async_trait]
impl IPaymentServiceFactory for FakeServiceFactory {
    async fn resolve(
        &self,
        _scope: &PaymentScope,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(FakeProvider {
            verifier: FakeVerifier,
        }))
    }

    async fn payment_options(
        &self,
        _tenant: Option<&webx_pay::TenantId>,
        _currency: Option<&webx_pay::Currency>,
    ) -> Result<Vec<PaymentOption>, PaymentError> {
        Ok(vec![PaymentOption::new(
            ProviderId::Stripe,
            webx_pay::AccountId::default_account(),
            webx_pay::PaymentMethod::Card,
            [webx_pay::PaymentFlowKind::ClientSdk],
        )])
    }
}

struct FakeProviderFactory;

#[async_trait]
impl IPaymentProviderFactory for FakeProviderFactory {
    fn provider(&self) -> ProviderId {
        ProviderId::Stripe
    }

    async fn resolve(
        &self,
        _scope: &PaymentScope,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        Ok(Arc::new(FakeProvider {
            verifier: FakeVerifier,
        }))
    }
}

struct FakeBuilder;

impl IPaymentProviderBuilder for FakeBuilder {
    fn provider(&self) -> ProviderId {
        ProviderId::Stripe
    }

    fn build(
        &self,
        _config: &PaymentProviderConfig,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        Ok(Arc::new(FakeProvider {
            verifier: FakeVerifier,
        }))
    }
}

#[tokio::test]
async fn custom_service_factory_replaces_the_default() {
    let factory = Arc::new(FakeServiceFactory {
        calls: AtomicUsize::new(0),
    });
    let registered = Arc::clone(&factory);
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        // Native DI registration: no payment-specific override API.
        .register(move |services| {
            services
                .instance::<dyn IPaymentServiceFactory>(registered)
                .ensure_payments()
        })
        .build();
    let payments = host.provider().get::<Payments>().expect("payments runtime");
    payments.resolve(ProviderId::Stripe).await.expect("resolve");
    assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn provider_factories_and_builders_are_collected() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(|services| {
            services
                .add_payment_provider_factory(Arc::new(FakeProviderFactory))
                .add_payment_provider_builder(Arc::new(FakeBuilder))
        })
        .build();

    let builders = host
        .provider()
        .get::<PaymentProviderBuilders>()
        .expect("builder registry");
    assert!(builders.contains(&ProviderId::Stripe));

    let payments = host.provider().get::<Payments>().expect("payments runtime");
    payments.resolve(ProviderId::Stripe).await.expect("resolve");
}

#[tokio::test]
async fn native_registration_after_adapters_overrides_the_default() {
    let factory = Arc::new(FakeServiceFactory {
        calls: AtomicUsize::new(0),
    });
    let registered = Arc::clone(&factory);
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        // Ensure the default factory exists first...
        .register(|services| {
            services
                .add_payment_provider_factory(Arc::new(FakeProviderFactory))
                .add_payment_provider_builder(Arc::new(FakeBuilder))
        })
        // ...then a normal DI registration overrides it (MEDI last wins).
        .register(move |services| services.instance::<dyn IPaymentServiceFactory>(registered))
        .build();
    let payments = host.provider().get::<Payments>().expect("payments runtime");
    payments.resolve(ProviderId::Stripe).await.expect("resolve");
    assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn catalog_lists_public_provider_options_for_the_tenant() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(|services| {
            services
                .add_payment_provider_factory(Arc::new(FakeProviderFactory))
                .add_payment_provider_builder(Arc::new(FakeBuilder))
        })
        .build();
    let payments = host.provider().get::<Payments>().expect("payments runtime");
    let options = payments.payment_options(None, None).await.expect("catalog");
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].provider, ProviderId::Stripe);
    assert_eq!(options[0].method, webx_pay::PaymentMethod::Card);

    // Currency filtering uses the provider's declared capabilities.
    let cny = webx_pay::Money::new(webx_pay::Currency::CNY, 100);
    assert_eq!(
        payments
            .payment_options(None, Some(&cny))
            .await
            .expect("cny catalog")
            .len(),
        1
    );
    let usd = webx_pay::Money::new(webx_pay::Currency::USD, 100);
    assert!(payments
        .payment_options(None, Some(&usd))
        .await
        .expect("usd catalog")
        .is_empty());

    let user = webx::IdentityUser {
        tenant_id: "tenant-a".to_string(),
        ..Default::default()
    };
    // The default factory is deployment-level and must fail closed for a
    // tenant-scoped catalog until the application registers a tenant-aware
    // factory.
    let error = payments.payment_options_for(&user, None).await.unwrap_err();
    assert_eq!(error.http_status(), 500);
}

#[tokio::test]
async fn custom_factory_owns_the_tenant_catalog() {
    let factory = Arc::new(FakeServiceFactory {
        calls: AtomicUsize::new(0),
    });
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(move |services| {
            services
                .instance::<dyn IPaymentServiceFactory>(factory)
                .ensure_payments()
        })
        .build();
    let payments = host.provider().get::<Payments>().expect("payments runtime");
    let user = webx::IdentityUser {
        tenant_id: "tenant-a".to_string(),
        ..Default::default()
    };
    let options = payments
        .payment_options_for(&user, None)
        .await
        .expect("tenant catalog");
    assert_eq!(options.len(), 1);
}

#[tokio::test]
async fn runtime_options_are_native_di_services() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(|services| {
            services
                .instance::<PaymentRuntimeOptions>(Arc::new(
                    PaymentRuntimeOptions::default()
                        .with_cache_ttl(Duration::from_secs(7))
                        .with_cache_capacity(32),
                ))
                .add_payment_provider_factory(Arc::new(FakeProviderFactory))
                .add_payment_provider_builder(Arc::new(FakeBuilder))
        })
        .build();
    let payments = host.provider().get::<Payments>().expect("payments runtime");
    assert_eq!(payments.options().cache_ttl, Duration::from_secs(7));
    assert_eq!(payments.options().cache_capacity, 32);
}

#[tokio::test]
async fn di_aware_service_factory_is_supported() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(|services| {
            services
                .singleton::<dyn IPaymentServiceFactory>(|_resolver| {
                    Arc::new(FakeServiceFactory {
                        calls: AtomicUsize::new(0),
                    })
                })
                .ensure_payments()
        })
        .build();
    let payments = host.provider().get::<Payments>().expect("payments runtime");
    payments.resolve(ProviderId::Stripe).await.expect("resolve");
}
