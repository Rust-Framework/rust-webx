//! Runtime payment factories.
//!
//! Three layers:
//!
//! - [`IPaymentProviderFactory`]: adapter-level source registered by
//!   `add_stripe()` / `add_stripe_with(options)`; turns a [`PaymentScope`] into
//!   a provider instance (from settings or a prebuilt static instance).
//! - [`IPaymentServiceFactory`]: application-level source; the default
//!   implementation routes to registered provider factories, an application may
//!   replace it to load configuration from a database/KMS.
//! - [`IPaymentProviderBuilder`]: lets a custom service factory reuse the
//!   official adapter construction from a [`PaymentProviderConfig`] without
//!   depending on the adapter crate.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde_json::Value;

use crate::error::PaymentError;
use crate::ids::{AccountId, ProviderId, TenantId};
use crate::provider::IPaymentProvider;
use crate::scope::PaymentScope;

/// Configuration envelope for [`IPaymentProviderBuilder`].
///
/// `settings` uses the same serde shape as the adapter's `XxxOptions`; a custom
/// service factory resolves secret references into it before calling the builder.
#[derive(Debug, Clone)]
pub struct PaymentProviderConfig {
    /// Provider the settings belong to.
    pub provider: ProviderId,
    /// Merchant account this configuration describes, when scoped to one.
    pub account: Option<AccountId>,
    /// Live mode vs sandbox, when the provider's settings do not carry it.
    pub live: Option<bool>,
    /// Adapter-specific options JSON (same shape as `XxxOptions`).
    pub settings: Value,
    /// Public callback URL configured for this provider/account.
    pub webhook_url: Option<String>,
    /// Opaque configuration version for logs/metrics.
    pub version: Option<String>,
}

impl PaymentProviderConfig {
    /// A configuration envelope with empty adapter settings.
    pub fn new(provider: ProviderId, settings: Value) -> Self {
        Self {
            provider,
            account: None,
            live: None,
            settings,
            webhook_url: None,
            version: None,
        }
    }
}

/// Builds a provider from runtime configuration.
///
/// Implemented by adapter crates. Custom [`IPaymentServiceFactory`]
/// implementations resolve this from the container to reuse the official
/// protocol implementation:
///
/// ```ignore
/// let provider = builders.build(scope.provider(), &config)?;
/// ```
pub trait IPaymentProviderBuilder: Send + Sync {
    /// Provider this builder constructs.
    fn provider(&self) -> ProviderId;

    /// Parse `config.settings` and build a validated provider.
    fn build(
        &self,
        config: &PaymentProviderConfig,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError>;

    /// Optional admin-UI example of the settings JSON.
    fn settings_example(&self) -> Option<Value> {
        None
    }
}

/// Adapter-level provider source registered by `add_*` on the host builder.
#[async_trait::async_trait]
pub trait IPaymentProviderFactory: Send + Sync {
    /// Provider served by this factory.
    fn provider(&self) -> ProviderId;

    /// Resolve a validated provider for `scope`.
    async fn resolve(
        &self,
        scope: &PaymentScope,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError>;

    /// Frontend catalog for this provider (optionally tenant/currency-scoped).
    ///
    /// The default builds a default-scope provider, asks it for its public
    /// options and filters by the provider's declared currency support;
    /// tenant-aware factories can override.
    async fn payment_options(
        &self,
        tenant: Option<&TenantId>,
        currency: Option<&crate::Currency>,
    ) -> Result<Vec<crate::PaymentOption>, PaymentError> {
        let mut scope = PaymentScope::new(self.provider());
        if let Some(tenant) = tenant {
            scope = scope.tenant(tenant.clone());
        }
        let provider = self.resolve(&scope).await?;
        let mut options = provider.payment_options();
        if let Some(currency) = currency {
            if !provider.capabilities().supports_currency(currency) {
                options.clear();
            }
        }
        Ok(options)
    }
}

/// Application-level payment source.
///
/// This is a **normal DI singleton**, not a payment-specific registration
/// mechanism. Implement it when configuration comes from a database, config
/// center or secret manager, then register it through the container:
///
/// ```ignore
/// Host::builder()
///     .add_stripe()
///     .register(|services| {
///         services.singleton::<dyn IPaymentServiceFactory>(|resolver| {
///             Arc::new(DbPaymentServiceFactory {
///                 scopes: resolver.resolve_arc::<dyn ScopeFactory>().expect("scopes"),
///                 builders: resolver
///                     .resolve_arc::<PaymentProviderBuilders>()
///                     .expect("builders"),
///             })
///         })
///     })
///     .build();
/// ```
///
/// The framework registers its default factory through `try_add_singleton`, so
/// a user registration before or after `add_*` overrides it using MEDI
/// last-registration-wins semantics. The runtime resolves this trait through
/// the container; there is no separate `use_payment_service_factory` API.
///
/// Responsibilities:
/// - resolve the **current** configuration for `scope` (tenant/account mapping,
///   enablement, credentials) and return a validated provider;
/// - enforce tenant/account authorization (the framework only isolates cache
///   keys, it cannot know business ownership);
/// - map configuration-source failures to [`PaymentError::Unavailable`];
/// - optionally implement [`Self::probe`] for readiness and
///   [`Self::invalidate`] if the factory keeps its own cache.
///
/// Not responsibilities: protocol signing/parsing (adapters/builders), provider
/// caching (done by [`crate::Payments`]), or webhook/event storage (inbox or the
/// application).
#[async_trait::async_trait]
pub trait IPaymentServiceFactory: Send + Sync {
    /// Resolve a validated provider for `scope`.
    async fn resolve(
        &self,
        scope: &PaymentScope,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError>;

    /// Readiness probe for the configuration source (DB/KMS reachability).
    ///
    /// The default is healthy; implementations backed by external systems
    /// should override it.
    async fn probe(&self) -> Result<(), PaymentError> {
        Ok(())
    }

    /// Optional invalidation hook for factories that keep their own cache.
    ///
    /// `Payments` already evicts its own cache before calling this.
    fn invalidate(&self, _scope: &PaymentScope) {}

    /// Frontend catalog for the current tenant (all enabled providers/accounts).
    ///
    /// The default implementation returns an empty list for custom factories;
    /// a DB-backed factory should override it to list enabled configurations.
    async fn payment_options(
        &self,
        _tenant: Option<&TenantId>,
        _currency: Option<&crate::Currency>,
    ) -> Result<Vec<crate::PaymentOption>, PaymentError> {
        Ok(Vec::new())
    }
}

/// Provider factory backed by a prebuilt instance (`add_*_with(options)`).
pub struct StaticPaymentProviderFactory {
    provider: Arc<dyn IPaymentProvider>,
}

impl StaticPaymentProviderFactory {
    /// Wrap a prebuilt provider.
    pub fn new(provider: Arc<dyn IPaymentProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait::async_trait]
impl IPaymentProviderFactory for StaticPaymentProviderFactory {
    fn provider(&self) -> ProviderId {
        self.provider.id()
    }

    async fn resolve(
        &self,
        _scope: &PaymentScope,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        Ok(Arc::clone(&self.provider))
    }
}

/// Registry of adapter builders, available to custom service factories.
#[derive(Default)]
pub struct PaymentProviderBuilders {
    builders: BTreeMap<ProviderId, Arc<dyn IPaymentProviderBuilder>>,
}

impl PaymentProviderBuilders {
    /// Build the registry from an iterator of builders (last one wins per id).
    pub fn new(builders: impl IntoIterator<Item = Arc<dyn IPaymentProviderBuilder>>) -> Self {
        Self {
            builders: builders
                .into_iter()
                .map(|builder| (builder.provider(), builder))
                .collect(),
        }
    }

    /// Whether `provider` has a registered builder.
    pub fn contains(&self, provider: &ProviderId) -> bool {
        self.builders.contains_key(provider)
    }

    /// Builder for `provider`.
    pub fn get(
        &self,
        provider: &ProviderId,
    ) -> Result<&Arc<dyn IPaymentProviderBuilder>, PaymentError> {
        self.builders.get(provider).ok_or_else(|| {
            PaymentError::not_found(provider.clone(), "provider builder not registered")
        })
    }

    /// Build a provider from a config envelope.
    pub fn build(
        &self,
        provider: &ProviderId,
        config: &PaymentProviderConfig,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        if config.provider != *provider {
            return Err(PaymentError::config(
                provider.clone(),
                "configuration provider does not match the requested provider",
            ));
        }
        self.get(provider)?.build(config)
    }

    /// Registered provider ids.
    pub fn provider_ids(&self) -> impl Iterator<Item = &ProviderId> {
        self.builders.keys()
    }
}

impl fmt::Debug for PaymentProviderBuilders {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentProviderBuilders")
            .field("providers", &self.providers())
            .finish()
    }
}

impl PaymentProviderBuilders {
    fn providers(&self) -> Vec<String> {
        self.builders.keys().map(ToString::to_string).collect()
    }
}

/// Default service factory: route each scope to its registered provider factory.
pub struct DefaultPaymentServiceFactory {
    providers: BTreeMap<ProviderId, Arc<dyn IPaymentProviderFactory>>,
}

impl DefaultPaymentServiceFactory {
    /// Build from registered provider factories (last one wins per id).
    pub fn new(providers: impl IntoIterator<Item = Arc<dyn IPaymentProviderFactory>>) -> Self {
        Self {
            providers: providers
                .into_iter()
                .map(|factory| (factory.provider(), factory))
                .collect(),
        }
    }

    /// Registered provider factories.
    pub fn provider_ids(&self) -> impl Iterator<Item = &ProviderId> {
        self.providers.keys()
    }
}

impl fmt::Debug for DefaultPaymentServiceFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DefaultPaymentServiceFactory")
            .field(
                "providers",
                &self
                    .providers
                    .keys()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[async_trait::async_trait]
impl IPaymentServiceFactory for DefaultPaymentServiceFactory {
    async fn resolve(
        &self,
        scope: &PaymentScope,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        let factory = self.providers.get(scope.provider()).ok_or_else(|| {
            PaymentError::not_found(
                scope.provider().clone(),
                "payment provider is not registered",
            )
        })?;
        factory.resolve(scope).await
    }

    async fn payment_options(
        &self,
        tenant: Option<&TenantId>,
        currency: Option<&crate::Currency>,
    ) -> Result<Vec<crate::PaymentOption>, PaymentError> {
        // The default factory reads deployment-level configuration (one set of
        // accounts for the whole process). It cannot safely attribute those
        // accounts to a tenant, so a tenant-scoped catalog must come from a
        // custom factory that owns the ownership mapping.
        if tenant.is_some() {
            return Err(PaymentError::config(
                ProviderId::Custom("payment-catalog".to_string()),
                "the default payment catalog is deployment-level; register a custom \
                 IPaymentServiceFactory that implements tenant-scoped payment_options",
            ));
        }
        let mut options = Vec::new();
        for factory in self.providers.values() {
            // One misconfigured provider must not hide the options of the
            // healthy ones; readiness/health checks surface the broken config.
            match factory.payment_options(None, currency).await {
                Ok(provider_options) => options.extend(provider_options),
                Err(error) => tracing::warn!(
                    provider = %factory.provider(),
                    error = %error,
                    "payment catalog skipped a misconfigured provider"
                ),
            }
        }
        options.sort_by(|left, right| {
            left.provider
                .cmp(&right.provider)
                .then_with(|| left.account.cmp(&right.account))
                .then_with(|| left.method.as_str().cmp(right.method.as_str()))
        });
        Ok(options)
    }
}
