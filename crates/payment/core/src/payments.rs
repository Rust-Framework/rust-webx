//! The `Payments` facade: cached runtime resolution + typed operations.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::error::PaymentError;
use crate::events::PaymentEvent;
use crate::factory::IPaymentServiceFactory;
use crate::ids::{ProviderId, TenantId};
use crate::model::{CreatePayment, PaymentAccount, PaymentIntent, Refund, RefundCommand};
use crate::money::Money;
use crate::provider::IPaymentProvider;
use crate::scope::PaymentScope;
use crate::webhook::PaymentWebhook;
use crate::PaymentOption;

/// Runtime cache/probe policy for [`Payments`].
#[derive(Debug, Clone)]
pub struct PaymentRuntimeOptions {
    /// How long a resolved provider is reused. Configuration/document changes
    /// become visible after at most this duration (or immediately after
    /// [`Payments::invalidate`]).
    pub cache_ttl: Duration,
    /// Upper bound on cached provider instances (tenant/account cardinality).
    pub cache_capacity: usize,
}

impl Default for PaymentRuntimeOptions {
    fn default() -> Self {
        Self {
            cache_ttl: Duration::from_secs(60),
            cache_capacity: 1024,
        }
    }
}

impl PaymentRuntimeOptions {
    /// Replace the cache TTL.
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// Replace the cache capacity.
    pub fn with_cache_capacity(mut self, capacity: usize) -> Self {
        self.cache_capacity = capacity.max(1);
        self
    }
}

struct CacheEntry {
    provider: Arc<dyn IPaymentProvider>,
    expires_at: Instant,
}

/// Application-facing payment facade.
///
/// Injected as `Arc<Payments>`; resolves providers through
/// [`IPaymentServiceFactory`], caches instances per [`PaymentScope`] and exposes
/// the typed operations plus raw webhook verification.
pub struct Payments {
    factory: Arc<dyn IPaymentServiceFactory>,
    cache: RwLock<HashMap<PaymentScope, CacheEntry>>,
    inflight: Mutex<HashMap<PaymentScope, Arc<Mutex<()>>>>,
    options: PaymentRuntimeOptions,
}

impl std::fmt::Debug for Payments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Payments")
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl Payments {
    /// Create a facade over a service factory.
    pub fn new(factory: Arc<dyn IPaymentServiceFactory>, options: PaymentRuntimeOptions) -> Self {
        Self {
            factory,
            cache: RwLock::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            options,
        }
    }

    /// The underlying runtime factory.
    pub fn factory(&self) -> &Arc<dyn IPaymentServiceFactory> {
        &self.factory
    }

    /// Runtime cache policy.
    pub fn options(&self) -> &PaymentRuntimeOptions {
        &self.options
    }

    /// Resolve (and cache) the provider for `scope`.
    ///
    /// Concurrent misses for the same scope share one factory call
    /// (single-flight); a failed factory call is never cached.
    pub async fn resolve(
        &self,
        scope: impl Into<PaymentScope>,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        let scope = scope.into();
        if let Some(provider) = self.cached(&scope) {
            return Ok(provider);
        }

        let gate = {
            let mut inflight = self.inflight.lock().await;
            Arc::clone(
                inflight
                    .entry(scope.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _guard = gate.lock().await;

        // Another caller may have populated the cache while we waited.
        if let Some(provider) = self.cached(&scope) {
            return Ok(provider);
        }

        let result = self.factory.resolve(&scope).await;
        self.remove_gate(&scope).await;
        let provider = result?;
        provider.validate()?;
        self.store(scope, Arc::clone(&provider));
        Ok(provider)
    }

    /// Drop the cached provider for `scope`.
    pub fn invalidate(&self, scope: impl Into<PaymentScope>) {
        let scope = scope.into();
        write_cache(&self.cache).remove(&scope);
        self.factory.invalidate(&scope);
    }

    /// Drop every cached provider of `provider`.
    pub fn invalidate_provider(&self, provider: &ProviderId) {
        write_cache(&self.cache).retain(|scope, _| scope.provider() != provider);
    }

    /// Drop the whole cache.
    pub fn clear(&self) {
        write_cache(&self.cache).clear();
    }

    /// Number of currently cached provider instances.
    pub fn cached_len(&self) -> usize {
        read_cache(&self.cache).len()
    }

    /// Probe the configuration source (used by readiness checks).
    pub async fn probe(&self) -> Result<(), PaymentError> {
        self.factory.probe().await
    }

    /// Create a payment.
    pub async fn create(
        &self,
        scope: impl Into<PaymentScope>,
        request: CreatePayment,
    ) -> Result<PaymentIntent, PaymentError> {
        request.validate()?;
        let scope = scope.into();
        let provider = self.resolve_ref(&scope).await?;
        provider.create_payment(&scope, request).await
    }

    /// Query a payment by merchant order id.
    pub async fn get(
        &self,
        scope: impl Into<PaymentScope>,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        let scope = scope.into();
        let provider = self.resolve_ref(&scope).await?;
        provider.get_payment(&scope, order_id).await
    }

    /// Capture an authorized payment.
    pub async fn capture(
        &self,
        scope: impl Into<PaymentScope>,
        order_id: &str,
        amount: Option<Money>,
    ) -> Result<PaymentIntent, PaymentError> {
        let scope = scope.into();
        let provider = self.resolve_ref(&scope).await?;
        provider.capture_payment(&scope, order_id, amount).await
    }

    /// Cancel/close/void a payment.
    pub async fn cancel(
        &self,
        scope: impl Into<PaymentScope>,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        let scope = scope.into();
        let provider = self.resolve_ref(&scope).await?;
        provider.cancel_payment(&scope, order_id).await
    }

    /// Refund a payment.
    pub async fn refund(
        &self,
        scope: impl Into<PaymentScope>,
        command: RefundCommand,
    ) -> Result<Refund, PaymentError> {
        command.validate()?;
        let scope = scope.into();
        let provider = self.resolve_ref(&scope).await?;
        provider.refund(&scope, command).await
    }

    /// Query a refund by merchant refund id.
    pub async fn get_refund(
        &self,
        scope: impl Into<PaymentScope>,
        refund_id: &str,
    ) -> Result<Refund, PaymentError> {
        let scope = scope.into();
        let provider = self.resolve_ref(&scope).await?;
        provider.get_refund(&scope, refund_id).await
    }

    /// Public account views served by the resolved provider.
    pub async fn accounts(
        &self,
        scope: impl Into<PaymentScope>,
    ) -> Result<Vec<PaymentAccount>, PaymentError> {
        let provider = self.resolve(scope).await?;
        Ok(provider.accounts().to_vec())
    }

    /// Frontend payment catalog for `tenant` (all configured providers/accounts).
    ///
    /// `currency` filters options by the provider's declared currency support.
    /// Returns public options only; secret keys never appear in the result.
    pub async fn payment_options(
        &self,
        tenant: Option<&TenantId>,
        currency: Option<&Money>,
    ) -> Result<Vec<PaymentOption>, PaymentError> {
        self.factory
            .payment_options(tenant, currency.map(|money| &money.currency))
            .await
    }

    /// Frontend payment catalog scoped by the authenticated identity.
    ///
    /// The tenant is read from [`webx_core::auth::IdentityUser::tenant_id`], the
    /// only trusted source; call this directly from the checkout-page endpoint.
    pub async fn payment_options_for(
        &self,
        user: &webx_core::auth::IdentityUser,
        currency: Option<&Money>,
    ) -> Result<Vec<PaymentOption>, PaymentError> {
        let tenant = PaymentScope::tenant_from_identity(user)?;
        self.payment_options(tenant.as_ref(), currency).await
    }

    /// Verify a raw webhook and normalize the event.
    pub async fn verify_webhook(
        &self,
        request: &PaymentWebhook,
    ) -> Result<PaymentEvent, PaymentError> {
        let scope = self.webhook_scope(request)?;
        let provider = self.resolve_ref(&scope).await?;
        provider
            .webhook_verifier()
            .verify(&scope, &request.payload())
            .await
    }

    /// The success acknowledgement for a verified webhook request.
    ///
    /// Uses the same `(provider, account)` scope as [`Self::verify_webhook`], so
    /// multi-account providers and tenant-aware factories resolve the same
    /// instance.
    pub async fn ack(
        &self,
        request: &PaymentWebhook,
    ) -> Result<webx_core::action::Response, PaymentError> {
        let scope = self.webhook_scope(request)?;
        let resolved = self.resolve_ref(&scope).await?;
        Ok(resolved.webhook_verifier().ack())
    }

    async fn resolve_ref(
        &self,
        scope: &PaymentScope,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        self.resolve(scope.clone()).await
    }

    fn webhook_scope(&self, request: &PaymentWebhook) -> Result<PaymentScope, PaymentError> {
        let mut scope = PaymentScope::new(request.provider_id()?);
        if let Some(account) = request.account_id()? {
            scope = scope.account(account);
        }
        Ok(scope)
    }

    fn cached(&self, scope: &PaymentScope) -> Option<Arc<dyn IPaymentProvider>> {
        let now = Instant::now();
        let cache = read_cache(&self.cache);
        let entry = cache.get(scope)?;
        if entry.expires_at > now {
            Some(Arc::clone(&entry.provider))
        } else {
            None
        }
    }

    fn store(&self, scope: PaymentScope, provider: Arc<dyn IPaymentProvider>) {
        let ttl = self.options.cache_ttl;
        if ttl.is_zero() {
            return;
        }
        let mut cache = write_cache(&self.cache);
        if cache.len() >= self.options.cache_capacity {
            if let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at)
                .map(|(scope, _)| scope.clone())
            {
                cache.remove(&oldest);
            }
        }
        cache.insert(
            scope,
            CacheEntry {
                provider,
                expires_at: Instant::now() + ttl,
            },
        );
    }

    async fn remove_gate(&self, scope: &PaymentScope) {
        let mut inflight = self.inflight.lock().await;
        inflight.remove(scope);
    }
}

fn read_cache(
    cache: &RwLock<HashMap<PaymentScope, CacheEntry>>,
) -> std::sync::RwLockReadGuard<'_, HashMap<PaymentScope, CacheEntry>> {
    cache
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_cache(
    cache: &RwLock<HashMap<PaymentScope, CacheEntry>>,
) -> std::sync::RwLockWriteGuard<'_, HashMap<PaymentScope, CacheEntry>> {
    cache
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use super::*;
    use crate::capabilities::ProviderCapabilities;
    use crate::factory::IPaymentServiceFactory;
    use crate::ids::AccountId;
    use crate::model::{CreatePayment, PaymentAccount, PaymentFlow, PaymentMethod};
    use crate::money::Money;
    use crate::provider::{IPaymentProvider, IPaymentWebhookVerifier, WebhookPayload};
    use crate::status::PaymentStatus;
    use crate::{PaymentError, PaymentEvent, ProviderId, WebhookCapability};

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
            crate::text_success_ack()
        }
    }

    struct FakeProvider {
        verifier: FakeVerifier,
    }

    impl FakeProvider {
        fn new() -> Self {
            Self {
                verifier: FakeVerifier,
            }
        }
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
            ProviderCapabilities::basic(WebhookCapability::HmacSha256)
        }

        fn validate(&self) -> Result<(), PaymentError> {
            Ok(())
        }

        async fn create_payment(
            &self,
            _scope: &PaymentScope,
            request: CreatePayment,
        ) -> Result<PaymentIntent, PaymentError> {
            let mut intent = PaymentIntent::new(
                ProviderId::Stripe,
                AccountId::default_account(),
                request.order_id,
                request.amount,
                PaymentStatus::Succeeded,
                "succeeded",
            );
            intent.provider_payment_id = Some("pi_test".to_string());
            Ok(intent)
        }

        async fn get_payment(
            &self,
            _scope: &PaymentScope,
            order_id: &str,
        ) -> Result<PaymentIntent, PaymentError> {
            Err(PaymentError::not_found(ProviderId::Stripe, order_id))
        }

        fn webhook_verifier(&self) -> &dyn IPaymentWebhookVerifier {
            &self.verifier
        }
    }

    struct FakeFactory {
        calls: AtomicUsize,
        failures: AtomicUsize,
        delay_ms: u64,
    }

    impl FakeFactory {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                failures: AtomicUsize::new(0),
                delay_ms: 0,
            })
        }
    }

    #[async_trait]
    impl IPaymentServiceFactory for FakeFactory {
        async fn resolve(
            &self,
            _scope: &PaymentScope,
        ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            }
            if self.failures.load(Ordering::SeqCst) > 0 {
                self.failures.fetch_sub(1, Ordering::SeqCst);
                return Err(PaymentError::unavailable(ProviderId::Stripe, "db down"));
            }
            Ok(Arc::new(FakeProvider::new()))
        }
    }

    fn facade_with(factory: Arc<FakeFactory>, ttl: Duration) -> Payments {
        Payments::new(
            factory,
            PaymentRuntimeOptions::default().with_cache_ttl(ttl),
        )
    }

    #[tokio::test]
    async fn resolution_is_cached_per_scope() {
        let factory = FakeFactory::new();
        let payments = facade_with(Arc::clone(&factory), Duration::from_secs(60));
        let scope = PaymentScope::new(ProviderId::Stripe).tenant("tenant-a");
        payments.resolve(scope.clone()).await.unwrap();
        payments.resolve(scope).await.unwrap();
        assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
        assert_eq!(payments.cached_len(), 1);
    }

    #[tokio::test]
    async fn tenants_do_not_share_cache_entries() {
        let factory = FakeFactory::new();
        let payments = facade_with(Arc::clone(&factory), Duration::from_secs(60));
        payments
            .resolve(PaymentScope::new(ProviderId::Stripe).tenant("a"))
            .await
            .unwrap();
        payments
            .resolve(PaymentScope::new(ProviderId::Stripe).tenant("b"))
            .await
            .unwrap();
        assert_eq!(factory.calls.load(Ordering::SeqCst), 2);
        assert_eq!(payments.cached_len(), 2);
    }

    #[tokio::test]
    async fn ttl_expiry_reloads_the_provider() {
        let factory = FakeFactory::new();
        let payments = facade_with(Arc::clone(&factory), Duration::from_millis(10));
        let scope = PaymentScope::new(ProviderId::Stripe);
        payments.resolve(scope.clone()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(25)).await;
        payments.resolve(scope).await.unwrap();
        assert_eq!(factory.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn invalidate_forces_a_fresh_resolution() {
        let factory = FakeFactory::new();
        let payments = facade_with(Arc::clone(&factory), Duration::from_secs(60));
        let scope = PaymentScope::new(ProviderId::Stripe);
        payments.resolve(scope.clone()).await.unwrap();
        payments.resolve(scope.clone()).await.unwrap();
        payments.invalidate(scope.clone());
        payments.resolve(scope).await.unwrap();
        assert_eq!(factory.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn concurrent_misses_share_one_factory_call() {
        let factory = Arc::new(FakeFactory {
            calls: AtomicUsize::new(0),
            failures: AtomicUsize::new(0),
            delay_ms: 20,
        });
        let payments = Arc::new(facade_with(Arc::clone(&factory), Duration::from_secs(60)));
        let scope = PaymentScope::new(ProviderId::Stripe);
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let payments = Arc::clone(&payments);
            let scope = scope.clone();
            tasks.push(tokio::spawn(async move {
                payments.resolve(scope).await.unwrap();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn factory_failures_are_not_cached() {
        let factory = Arc::new(FakeFactory {
            calls: AtomicUsize::new(0),
            failures: AtomicUsize::new(1),
            delay_ms: 0,
        });
        let payments = facade_with(Arc::clone(&factory), Duration::from_secs(60));
        let scope = PaymentScope::new(ProviderId::Stripe);
        let error = payments
            .resolve(scope.clone())
            .await
            .err()
            .expect("first resolve must fail");
        assert_eq!(error.http_status(), 503);
        assert!(error.retryable());
        payments.resolve(scope).await.unwrap();
        assert_eq!(factory.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn create_uses_the_resolved_provider_and_scope() {
        let factory = FakeFactory::new();
        let payments = facade_with(factory, Duration::from_secs(60));
        let scope = PaymentScope::new(ProviderId::Stripe).account("acct_1");
        let request = CreatePayment::new(
            "order-1",
            Money::cny(100),
            PaymentMethod::Card,
            PaymentFlow::ClientSdk { return_url: None },
        );
        let intent = payments.create(scope, request).await.unwrap();
        assert_eq!(intent.order_id, "order-1");
    }
}
