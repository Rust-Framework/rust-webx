//! Adyen Checkout provider, operations and host registration.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use rust_dix::IServiceResolverExt;
use webx_client::{HttpClient, HttpClientOptions, IHttpClient};
use webx_host::health::HealthCheckSpec;
use webx_host::health::HealthStatus;
use webx_host::server::HostBuilder;
use webx_pay::{
    AccountId, AccountSet, CaptureMode, CreatePayment, CurrencyScope, IPaymentProvider,
    IPaymentProviderBuilder, IPaymentProviderFactory, IPaymentWebhookVerifier, Money,
    PaymentAccount, PaymentError, PaymentFlow, PaymentFlowKind, PaymentIntent, PaymentMethod,
    PaymentOption, PaymentProviderConfig, PaymentScope, PaymentStatus,
    PaymentsServiceCollectionExt, ProviderCapabilities, ProviderId, Refund, RefundCommand,
    WebhookCapability,
};

use crate::mapping;
use crate::options::AdyenOptions;
use crate::transport;
use crate::webhook::AdyenWebhookVerifier;

const PROVIDER: ProviderId = ProviderId::Adyen;

/// Adyen idempotency keys are limited to 64 characters.
const MAX_IDEMPOTENCY_KEY_LEN: usize = 64;

/// Adyen pspReference length (16 alphanumeric characters).
const PSP_REFERENCE_LEN: usize = 16;

/// Static capability text for the unsupported generic payment query.
const QUERY_UNSUPPORTED: &str = "query (Adyen Checkout has no GET payment-status endpoint; call AdyenProvider::get_payment_with_details with the shopper's redirect/3-D Secure return details, or consume the AUTHORISATION webhook)";

/// Static capability text for the unsupported refund query.
const GET_REFUND_UNSUPPORTED: &str = "get_refund (Adyen refunds are asynchronous and there is no documented refund-status endpoint; consume the REFUND/REFUND_FAILED webhooks)";

/// One merchant account's credentials and HTTP client configuration.
#[derive(Clone)]
pub(crate) struct AdyenAccount {
    pub(crate) id: AccountId,
    pub(crate) api_key: String,
    pub(crate) merchant_account: String,
    /// Decoded hexadecimal HMAC key of the standard webhook endpoint.
    pub(crate) hmac_key: Vec<u8>,
    /// Public client key for Drop-in/Components; the only credential published
    /// to frontends through the payment catalog.
    pub(crate) client_key: Option<String>,
    pub(crate) live: bool,
    pub(crate) base_url: String,
    pub(crate) http: HttpClientOptions,
    pub(crate) client: OnceLock<Result<Arc<dyn IHttpClient>, String>>,
}

impl fmt::Debug for AdyenAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdyenAccount")
            .field("id", &self.id)
            .field("api_key", &"<redacted>")
            .field("merchant_account", &self.merchant_account)
            .field("hmac_key", &"<redacted>")
            .field(
                "client_key",
                &self.client_key.as_ref().map(|_| "<redacted>"),
            )
            .field("live", &self.live)
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// Adyen Checkout adapter.
///
/// Owns the configured merchant accounts, the outbound HTTP clients and the
/// webhook verifier. All provider-specific protocol details stay in this crate;
/// applications use it through [`IPaymentProvider`] or [`webx_pay::Payments`],
/// including the public frontend catalog served by
/// [`IPaymentProvider::payment_options`].
pub struct AdyenProvider {
    options: AdyenOptions,
    accounts: Result<AccountSet<AdyenAccount>, String>,
    verifier: AdyenWebhookVerifier,
    injected_client: Option<Arc<dyn IHttpClient>>,
    validated: OnceLock<Result<(), String>>,
}

impl AdyenProvider {
    /// Build a provider from options (configuration is validated at startup).
    pub fn new(options: AdyenOptions) -> Self {
        let accounts = options.build_accounts().map_err(|error| error.to_string());
        let verifier = AdyenWebhookVerifier::new(accounts.clone());
        Self {
            options,
            accounts,
            verifier,
            injected_client: None,
            validated: OnceLock::new(),
        }
    }

    /// Build a provider with an explicit HTTP client (tests, shared pool).
    pub fn with_client(options: AdyenOptions, client: Arc<dyn IHttpClient>) -> Self {
        let mut provider = Self::new(options);
        provider.injected_client = Some(client);
        provider
    }

    /// The options this provider was built from.
    pub fn options(&self) -> &AdyenOptions {
        &self.options
    }

    /// Synchronous health probe: configuration parses and the HTTP clients build.
    pub fn health(&self) -> HealthStatus {
        match self.validate() {
            Ok(()) => HealthStatus::pass(),
            Err(error) => HealthStatus::fail(error.to_string()),
        }
    }

    fn account_set(&self) -> Result<&AccountSet<AdyenAccount>, PaymentError> {
        self.accounts
            .as_ref()
            .map_err(|error| PaymentError::config(PROVIDER, error.clone()))
    }

    fn account_for(
        &self,
        requested: Option<&AccountId>,
    ) -> Result<&Arc<AdyenAccount>, PaymentError> {
        self.account_set()?.resolve(requested)
    }

    fn client_for(&self, account: &AdyenAccount) -> Result<Arc<dyn IHttpClient>, PaymentError> {
        if let Some(client) = &self.injected_client {
            return Ok(Arc::clone(client));
        }
        let built = account.client.get_or_init(|| {
            HttpClient::new("adyen", account.http.clone())
                .map(|client| Arc::new(client) as Arc<dyn IHttpClient>)
                .map_err(|error| error.to_string())
        });
        built
            .as_ref()
            .map(Arc::clone)
            .map_err(|error| PaymentError::config(PROVIDER, error.clone()))
    }

    fn path(&self, endpoint: &str) -> String {
        self.options.api_path(endpoint)
    }

    fn parse_payment_response(
        &self,
        account: &AdyenAccount,
        value: &serde_json::Value,
        fallback_order_id: &str,
        fallback_amount: Option<&Money>,
        flow: &PaymentFlow,
        capture: Option<CaptureMode>,
    ) -> Result<PaymentIntent, PaymentError> {
        let result_code = mapping::string_field(value, "resultCode").ok_or_else(|| {
            PaymentError::Serialization("adyen response is missing `resultCode`".to_string())
        })?;
        let amount = mapping::money_from_response(value, fallback_amount).ok_or_else(|| {
            PaymentError::Serialization("adyen response is missing `amount`".to_string())
        })?;
        let order_id = mapping::string_field(value, "merchantReference")
            .map(str::to_string)
            .unwrap_or_else(|| fallback_order_id.to_string());
        let status = mapping::map_result_code(result_code, capture);
        let mut intent = PaymentIntent::new(
            PROVIDER,
            account.id.clone(),
            order_id,
            amount,
            status,
            result_code.to_string(),
        );
        intent.provider_payment_id =
            mapping::string_field(value, "pspReference").map(str::to_string);
        intent.amount_captured = status.is_paid().then_some(amount);
        intent.client_action = mapping::client_action(flow, value);
        intent.created_at = crate::unix_now();
        Ok(intent)
    }

    fn parse_modification_payment(
        &self,
        account: &AdyenAccount,
        value: &serde_json::Value,
        order_id: &str,
        requested_amount: Option<&Money>,
    ) -> Result<PaymentIntent, PaymentError> {
        let status_raw = mapping::string_field(value, "status").unwrap_or("received");
        // Capture responses echo the captured amount; cancel responses carry no
        // amount at all, so they fall back to the documented `XXX` placeholder.
        let amount = mapping::money_from_response(value, requested_amount)
            .unwrap_or_else(mapping::unknown_amount);
        let mut intent = PaymentIntent::new(
            PROVIDER,
            account.id.clone(),
            order_id.to_string(),
            amount,
            mapping::map_modification_status(status_raw),
            status_raw.to_string(),
        );
        intent.provider_payment_id =
            mapping::string_field(value, "paymentPspReference").map(str::to_string);
        intent.created_at = crate::unix_now();
        Ok(intent)
    }

    fn parse_refund(
        &self,
        account: &AdyenAccount,
        value: &serde_json::Value,
        request: &RefundCommand,
    ) -> Result<Refund, PaymentError> {
        let status_raw = mapping::string_field(value, "status").unwrap_or("received");
        let amount =
            mapping::money_from_response(value, request.amount.as_ref()).ok_or_else(|| {
                PaymentError::Serialization("adyen refund response is missing `amount`".to_string())
            })?;
        Ok(Refund {
            provider: PROVIDER,
            account: account.id.clone(),
            order_id: request.order_id.clone(),
            refund_id: request.refund_id.clone(),
            provider_refund_id: mapping::string_field(value, "pspReference").map(str::to_string),
            status: mapping::map_refund_status(status_raw),
            provider_status: status_raw.to_string(),
            amount,
            created_at: crate::unix_now(),
            succeeded_at: None,
            request_id: None,
        })
    }
}

/// Public client configuration for the Adyen Drop-in/Components SDK.
///
/// Only frontend-safe identifiers are exposed: the client key (when
/// configured), the environment and the merchant account code. The Checkout API
/// key and the webhook HMAC key are server-side credentials and never appear
/// here.
fn public_client_config(account: &AdyenAccount) -> serde_json::Value {
    let mut config = serde_json::Map::new();
    config.insert(
        "environment".to_string(),
        serde_json::Value::String(if account.live { "live" } else { "test" }.to_string()),
    );
    config.insert(
        "merchant_account".to_string(),
        serde_json::Value::String(account.merchant_account.clone()),
    );
    if let Some(client_key) = &account.client_key {
        config.insert(
            "client_key".to_string(),
            serde_json::Value::String(client_key.clone()),
        );
    }
    serde_json::Value::Object(config)
}

/// Public frontend catalog for a validated account set.
///
/// Every configured account contributes one option per supported frontend
/// method, each with the client-SDK and redirect flows. Options carry only
/// public client configuration ([`public_client_config`]) and the operator label
/// from the public account view.
fn frontend_options(accounts: &AccountSet<AdyenAccount>) -> Vec<PaymentOption> {
    let mut options = Vec::new();
    for (account_id, account) in accounts.iter() {
        let label = accounts
            .accounts()
            .iter()
            .find(|info| &info.id == account_id)
            .and_then(|info| info.label.clone());
        let client_config = public_client_config(account);
        for method in [
            PaymentMethod::Card,
            PaymentMethod::ApplePay,
            PaymentMethod::GooglePay,
            PaymentMethod::PayPal,
        ] {
            let mut option = PaymentOption::new(
                PROVIDER,
                account_id.clone(),
                method,
                [PaymentFlowKind::ClientSdk, PaymentFlowKind::Redirect],
            )
            .with_client_config(client_config.clone());
            if let Some(label) = &label {
                option = option.with_label(label.clone());
            }
            options.push(option);
        }
    }
    options
}

#[async_trait]
impl IPaymentProvider for AdyenProvider {
    fn id(&self) -> ProviderId {
        PROVIDER
    }

    fn accounts(&self) -> &[PaymentAccount] {
        match &self.accounts {
            Ok(accounts) => accounts.accounts(),
            Err(_) => &[],
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            create: true,
            // Adyen exposes no generic GET payment-status endpoint; a status
            // re-query is only possible through `get_payment_with_details` with
            // the shopper's return details (see the crate docs).
            query: false,
            capture: true,
            cancel: true,
            refund: true,
            partial_refund: true,
            hosted_checkout: false,
            qr: true,
            app: true,
            webhook: WebhookCapability::HmacSha256,
            currencies: CurrencyScope::IsoAll,
            statuses: &[
                PaymentStatus::RequiresAction,
                PaymentStatus::Processing,
                PaymentStatus::Authorized,
                PaymentStatus::Succeeded,
                PaymentStatus::Canceled,
                PaymentStatus::Failed,
            ],
        }
    }

    /// Adyen Checkout methods enabled for every configured account.
    ///
    /// Card, Apple Pay, Google Pay and PayPal are offered through the client SDK
    /// and redirect flows; the attached `client_config` is public-only.
    fn payment_options(&self) -> Vec<PaymentOption> {
        match &self.accounts {
            Ok(accounts) => frontend_options(accounts),
            Err(_) => Vec::new(),
        }
    }

    fn validate(&self) -> Result<(), PaymentError> {
        let result = self.validated.get_or_init(|| {
            let accounts = self.account_set().map_err(|error| error.to_string())?;
            for id in accounts.ids() {
                let account = accounts
                    .resolve(Some(id))
                    .map_err(|error| error.to_string())?;
                self.client_for(account)
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        });
        result
            .as_ref()
            .map(|_| ())
            .map_err(|error| PaymentError::config(PROVIDER, error.clone()))
    }

    async fn create_payment(
        &self,
        scope: &PaymentScope,
        request: CreatePayment,
    ) -> Result<PaymentIntent, PaymentError> {
        request.validate()?;
        let account = self.account_for(scope.account_ref())?;
        let body = mapping::build_create_body(&request, &account.merchant_account)?;
        let client = self.client_for(account)?;
        let idempotency = idempotency_key(request.idempotency_key.as_deref(), &request.order_id)?;
        let value = transport::post_json(
            client.as_ref(),
            &account.api_key,
            self.path("/payments"),
            &body,
            Some(&idempotency),
        )
        .await?;
        let mut intent = self.parse_payment_response(
            account,
            &value,
            &request.order_id,
            Some(&request.amount),
            &request.flow,
            Some(request.capture),
        )?;
        intent.metadata = request.metadata;
        Ok(intent)
    }

    async fn get_payment(
        &self,
        _scope: &PaymentScope,
        _order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        // Adyen has no documented payment-status query endpoint; inventing one
        // would silently return wrong state. The honest failure points at the
        // supported provider-specific method.
        Err(PaymentError::unsupported(PROVIDER, QUERY_UNSUPPORTED))
    }

    async fn capture_payment(
        &self,
        scope: &PaymentScope,
        order_id: &str,
        amount: Option<Money>,
    ) -> Result<PaymentIntent, PaymentError> {
        let account = self.account_for(scope.account_ref())?;
        let psp_reference = payment_reference(order_id)?;
        let amount = amount.ok_or_else(|| {
            PaymentError::validation(
                "adyen capture requires an explicit amount (Adyen captures a specified amount and this adapter cannot query the authorized amount)",
            )
        })?;
        if !amount.is_positive() {
            return Err(PaymentError::validation("capture amount must be positive"));
        }
        let body = mapping::build_capture_body(&account.merchant_account, &amount, psp_reference);
        let client = self.client_for(account)?;
        let idempotency = idempotency_key(None, psp_reference)?;
        let value = transport::post_json(
            client.as_ref(),
            &account.api_key,
            self.path(&format!("/payments/{psp_reference}/captures")),
            &body,
            Some(&idempotency),
        )
        .await?;
        self.parse_modification_payment(account, &value, order_id, Some(&amount))
    }

    async fn cancel_payment(
        &self,
        scope: &PaymentScope,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        let account = self.account_for(scope.account_ref())?;
        let psp_reference = payment_reference(order_id)?;
        let body = mapping::build_cancel_body(&account.merchant_account, psp_reference);
        let client = self.client_for(account)?;
        let idempotency = idempotency_key(None, psp_reference)?;
        let value = transport::post_json(
            client.as_ref(),
            &account.api_key,
            self.path(&format!("/payments/{psp_reference}/cancels")),
            &body,
            Some(&idempotency),
        )
        .await?;
        self.parse_modification_payment(account, &value, order_id, None)
    }

    async fn refund(
        &self,
        scope: &PaymentScope,
        request: RefundCommand,
    ) -> Result<Refund, PaymentError> {
        request.validate()?;
        let account = self.account_for(scope.account_ref())?;
        let psp_reference = payment_reference(&request.order_id)?;
        // Adyen's refund request requires an explicit amount; a full refund is
        // still expressed as an amount, which this adapter cannot look up.
        let amount = request.amount.ok_or_else(|| {
            PaymentError::validation(
                "adyen refund requires an explicit amount (Adyen refunds a specified amount and this adapter cannot query the captured amount)",
            )
        })?;
        let body =
            mapping::build_refund_body(&account.merchant_account, &amount, &request.refund_id)?;
        let client = self.client_for(account)?;
        let idempotency = idempotency_key(request.idempotency_key.as_deref(), &request.refund_id)?;
        let value = transport::post_json(
            client.as_ref(),
            &account.api_key,
            self.path(&format!("/payments/{psp_reference}/refunds")),
            &body,
            Some(&idempotency),
        )
        .await?;
        self.parse_refund(account, &value, &request)
    }

    async fn get_refund(
        &self,
        _scope: &PaymentScope,
        _refund_id: &str,
    ) -> Result<Refund, PaymentError> {
        Err(PaymentError::unsupported(PROVIDER, GET_REFUND_UNSUPPORTED))
    }

    fn webhook_verifier(&self) -> &dyn IPaymentWebhookVerifier {
        &self.verifier
    }
}

impl AdyenProvider {
    /// Complete a payment with the details the shopper returned with
    /// (`POST /v71/payments/details`).
    ///
    /// This is the only documented way to learn the outcome of a redirect or
    /// 3-D Secure flow in the Checkout API. `details` is the object the client
    /// collected from the return URL (for example `{"redirectResult": "..."}`
    /// or `{"threeDSResult": "..."}`); `payment_data` is the optional
    /// `paymentData` returned in the `/payments` action.
    ///
    /// The generic [`IPaymentProvider::get_payment`] cannot express these
    /// details, so it returns `Unsupported` instead of guessing.
    pub async fn get_payment_with_details(
        &self,
        account: Option<&AccountId>,
        order_id: &str,
        details: serde_json::Value,
        payment_data: Option<String>,
    ) -> Result<PaymentIntent, PaymentError> {
        if order_id.trim().is_empty() {
            return Err(PaymentError::validation("order_id must not be empty"));
        }
        let account = self.account_for(account)?;
        let body = mapping::build_details_body(&details, payment_data.as_deref())?;
        let client = self.client_for(account)?;
        let value = transport::post_json(
            client.as_ref(),
            &account.api_key,
            self.path("/payments/details"),
            &body,
            None,
        )
        .await?;
        self.parse_payment_response(
            account,
            &value,
            order_id,
            None,
            &PaymentFlow::ClientSdk { return_url: None },
            None,
        )
    }

    /// Like [`AdyenProvider::get_payment_with_details`], reading the JSON
    /// `details` object from the `adyen_details` metadata key and the optional
    /// `adyen_payment_data` value.
    ///
    /// Applications that stored the shopper's return payload in their order
    /// metadata can hand that map straight to this method.
    pub async fn get_payment_with_metadata(
        &self,
        account: Option<&AccountId>,
        order_id: &str,
        metadata: &BTreeMap<String, String>,
    ) -> Result<PaymentIntent, PaymentError> {
        let raw = metadata
            .get(crate::METADATA_DETAILS)
            .ok_or_else(|| PaymentError::unsupported(PROVIDER, QUERY_UNSUPPORTED))?;
        let details: serde_json::Value = serde_json::from_str(raw).map_err(|error| {
            PaymentError::validation(format!(
                "metadata `{}` must be a JSON object of Adyen payment details: {error}",
                crate::METADATA_DETAILS
            ))
        })?;
        let payment_data = metadata.get(crate::METADATA_PAYMENT_DATA).cloned();
        self.get_payment_with_details(account, order_id, details, payment_data)
            .await
    }
}

/// Extract and validate the 16-character Adyen pspReference carried in `order_id`.
///
/// The shared port carries no separate provider-payment-id parameter, and Adyen
/// has no lookup-by-merchant-reference endpoint, so capture/cancel/refund must
/// receive [`PaymentIntent::provider_payment_id`] from the payment creation
/// (a `psp:` prefix is accepted for readability).
pub(crate) fn payment_reference(order_id: &str) -> Result<&str, PaymentError> {
    let value = order_id.trim();
    let value = value.strip_prefix("psp:").unwrap_or(value);
    let valid =
        value.len() == PSP_REFERENCE_LEN && value.bytes().all(|byte| byte.is_ascii_alphanumeric());
    if !valid {
        return Err(PaymentError::validation(
            "adyen capture/cancel/refund require the 16-character payment pspReference in `order_id` (Adyen has no lookup-by-merchant-reference endpoint; pass PaymentIntent.provider_payment_id)",
        ));
    }
    Ok(value)
}

fn idempotency_key(candidate: Option<&str>, fallback: &str) -> Result<String, PaymentError> {
    let key = candidate.unwrap_or(fallback).trim();
    if key.is_empty() || key.len() > MAX_IDEMPOTENCY_KEY_LEN || key.chars().any(char::is_control) {
        return Err(PaymentError::validation(format!(
            "adyen idempotency keys must be 1-{MAX_IDEMPOTENCY_KEY_LEN} characters without control characters"
        )));
    }
    Ok(key.to_string())
}

/// Register Adyen on a [`HostBuilder`].
pub trait AdyenHostBuilderExt: Sized {
    /// Register the Adyen adapter with runtime configuration.
    ///
    /// Binds the `"Adyen"` configuration section through `add_options_monitor`,
    /// so appsettings/file reloads are picked up by the runtime cache (default
    /// 60s) or immediately after `Payments::invalidate`.
    fn add_adyen(self) -> Self;

    /// Register the adapter with a prebuilt static configuration.
    fn add_adyen_with(self, options: AdyenOptions) -> Self;
}

impl AdyenHostBuilderExt for HostBuilder {
    fn add_adyen(self) -> Self {
        let builder = self.add_options_monitor::<AdyenOptions>("Adyen");
        builder.register(|services| {
            services
                .add_payment_provider_factory_lazy(|resolver| {
                    let monitor = resolver
                        .resolve_arc::<
                            dyn webx_core::options_monitor::IOptionsMonitor<AdyenOptions>,
                        >()
                        .expect("add_adyen() registers the Adyen options monitor");
                    Arc::new(AdyenProviderFactory::new(monitor))
                })
                .add_payment_provider_builder(Arc::new(AdyenProviderBuilder))
        })
    }

    fn add_adyen_with(self, options: AdyenOptions) -> Self {
        let provider = Arc::new(AdyenProvider::new(options));
        let health = Arc::clone(&provider);
        self.register(move |services| {
            services
                .add_payment_provider(provider)
                .add_payment_provider_builder(Arc::new(AdyenProviderBuilder))
        })
        .add_health_check(HealthCheckSpec::sync("payments.adyen", move || {
            health.health()
        }))
    }
}

/// Runtime provider factory for Adyen (reads the options monitor).
pub struct AdyenProviderFactory {
    monitor: Arc<dyn webx_core::options_monitor::IOptionsMonitor<AdyenOptions>>,
}

impl AdyenProviderFactory {
    /// Build the factory over an options monitor.
    pub fn new(
        monitor: Arc<dyn webx_core::options_monitor::IOptionsMonitor<AdyenOptions>>,
    ) -> Self {
        Self { monitor }
    }
}

#[async_trait]
impl IPaymentProviderFactory for AdyenProviderFactory {
    fn provider(&self) -> ProviderId {
        PROVIDER
    }

    async fn resolve(
        &self,
        _scope: &PaymentScope,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        let options = self.monitor.current();
        let provider = AdyenProvider::new((*options).clone());
        provider.validate()?;
        Ok(Arc::new(provider))
    }
}

/// Builder used by custom `IPaymentServiceFactory` implementations.
#[derive(Debug, Default, Clone, Copy)]
pub struct AdyenProviderBuilder;

impl IPaymentProviderBuilder for AdyenProviderBuilder {
    fn provider(&self) -> ProviderId {
        PROVIDER
    }

    fn build(
        &self,
        config: &PaymentProviderConfig,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        let options: AdyenOptions =
            serde_json::from_value(config.settings.clone()).map_err(|error| {
                PaymentError::config(
                    PROVIDER,
                    format!("invalid Adyen provider settings: {error}"),
                )
            })?;
        let provider = AdyenProvider::new(options);
        provider.validate()?;
        Ok(Arc::new(provider))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::AdyenAccountOptions;
    use serde_json::json;
    use webx_client::testing::MockUpstream;
    use webx_pay::{Currency, PaymentFlowKind, PaymentMethod};

    const HMAC_FIXTURE: &str = "00112233445566778899aabbccddeeff";
    const PSP_FIXTURE: &str = "7914073381342284";

    fn test_options() -> AdyenOptions {
        AdyenOptions::new("fixture-api-key", "FixtureMerchant", HMAC_FIXTURE)
    }

    fn test_provider() -> AdyenProvider {
        AdyenProvider::new(test_options())
    }

    fn mock_options(upstream: &MockUpstream) -> AdyenOptions {
        test_options()
            .with_test_api_base(upstream.base_url())
            .allow_private_networks(true)
    }

    fn scope() -> PaymentScope {
        PaymentScope::new(PROVIDER)
    }

    fn redirect_request() -> CreatePayment {
        CreatePayment::new(
            "ORDER-1",
            Money::new(Currency::EUR, 9900),
            PaymentMethod::Other("ideal".into()),
            PaymentFlow::Redirect {
                success_url: "https://merchant.example.com/return".into(),
                cancel_url: None,
            },
        )
    }

    #[test]
    fn account_debug_never_prints_credentials() {
        let provider = test_provider();
        let account = provider.account_for(None).unwrap();
        let text = format!("{account:?}");
        assert!(!text.contains("fixture-api-key"));
        assert!(!text.contains(HMAC_FIXTURE));
        assert!(text.contains("<redacted>"));
        assert!(text.contains("FixtureMerchant"));
    }

    #[test]
    fn payment_options_cover_the_public_methods_of_every_account() {
        let provider = test_provider();
        let options = provider.payment_options();
        assert_eq!(options.len(), 4);
        let methods: Vec<PaymentMethod> =
            options.iter().map(|option| option.method.clone()).collect();
        assert_eq!(
            methods,
            vec![
                PaymentMethod::Card,
                PaymentMethod::ApplePay,
                PaymentMethod::GooglePay,
                PaymentMethod::PayPal,
            ]
        );
        for option in &options {
            assert_eq!(option.provider, PROVIDER);
            assert_eq!(option.account.as_str(), "default");
            assert_eq!(
                option.flows,
                [PaymentFlowKind::ClientSdk, PaymentFlowKind::Redirect]
            );
            assert!(option.label.is_none());
        }
    }

    #[test]
    fn payment_options_publish_only_public_client_configuration() {
        let mut options = AdyenOptions::new("fixture-api-key", "FixtureMerchant", HMAC_FIXTURE);
        options.accounts[0].client_key = Some("fixture-client-key".to_string());
        let provider = AdyenProvider::new(options);

        let listed = provider.payment_options();
        assert_eq!(listed.len(), 4);
        for option in &listed {
            let config = option
                .client_config
                .as_ref()
                .expect("every adyen option carries the public client config");
            assert_eq!(config["merchant_account"], json!("FixtureMerchant"));
            assert_eq!(config["environment"], json!("test"));
            assert_eq!(config["client_key"], json!("fixture-client-key"));
            assert!(config.get("api_key").is_none());
            assert!(config.get("hmac_key").is_none());

            let serialized = serde_json::to_string(option).unwrap();
            assert!(!serialized.contains("fixture-api-key"));
            assert!(!serialized.contains(HMAC_FIXTURE));
            assert!(!serialized.contains("hmac"));
        }
    }

    #[test]
    fn payment_options_omit_client_key_and_mark_the_environment() {
        let options = AdyenOptions::new("fixture-api-key", "FixtureMerchant", HMAC_FIXTURE)
            .with_account(
                AdyenAccountOptions::new("live-api-key", "LiveMerchant", HMAC_FIXTURE)
                    .with_id("live")
                    .with_live(true)
                    .with_live_url_prefix("fixture-prefix"),
            )
            .with_default_account("default");
        let provider = AdyenProvider::new(options);

        let listed = provider.payment_options();
        assert_eq!(listed.len(), 8);

        let test = listed
            .iter()
            .find(|option| option.account.as_str() == "default")
            .expect("test account options");
        let config = test.client_config.as_ref().unwrap();
        assert!(config.get("client_key").is_none());
        assert_eq!(config["environment"], json!("test"));
        assert_eq!(config["merchant_account"], json!("FixtureMerchant"));

        let live = listed
            .iter()
            .find(|option| option.account.as_str() == "live")
            .expect("live account options");
        let config = live.client_config.as_ref().unwrap();
        assert!(config.get("client_key").is_none());
        assert_eq!(config["environment"], json!("live"));
        assert_eq!(config["merchant_account"], json!("LiveMerchant"));
        assert!(!serde_json::to_string(live)
            .unwrap()
            .contains("live-api-key"));
    }

    #[test]
    fn payment_options_carry_the_public_account_label() {
        let built = test_options().build_accounts().unwrap();
        let id = built.default_id().clone();
        let credentials = built.resolve(None).unwrap().as_ref().clone();
        let set = AccountSet::new(
            PROVIDER,
            id.clone(),
            [(
                id.clone(),
                credentials,
                PaymentAccount {
                    id,
                    provider: PROVIDER,
                    live: false,
                    label: Some("Fixture account".to_string()),
                },
            )],
        )
        .unwrap();

        let options = frontend_options(&set);
        assert_eq!(options.len(), 4);
        assert!(options
            .iter()
            .all(|option| option.label.as_deref() == Some("Fixture account")));
    }

    #[test]
    fn payment_options_are_empty_for_invalid_configuration() {
        let provider = AdyenProvider::new(AdyenOptions::empty());
        assert!(provider.payment_options().is_empty());
    }

    #[test]
    fn psp_references_are_validated_before_they_reach_a_path() {
        assert_eq!(payment_reference(PSP_FIXTURE).unwrap(), PSP_FIXTURE);
        assert_eq!(
            payment_reference(&format!("psp:{PSP_FIXTURE}")).unwrap(),
            PSP_FIXTURE
        );
        assert!(payment_reference("../etc/passwd").is_err());
        assert!(payment_reference("ORDER-1").is_err());
        assert!(payment_reference("").is_err());
    }

    #[test]
    fn idempotency_keys_fall_back_to_the_order_or_refund_id() {
        assert_eq!(
            idempotency_key(None, "ORDER-1").unwrap(),
            "ORDER-1".to_string()
        );
        assert_eq!(
            idempotency_key(Some("explicit-key"), "ORDER-1").unwrap(),
            "explicit-key".to_string()
        );
        assert!(idempotency_key(None, &"x".repeat(65)).is_err());
        assert!(idempotency_key(Some("bad\nkey"), "ORDER-1").is_err());
    }

    #[test]
    fn parses_payment_responses_into_the_shared_model() {
        let provider = test_provider();
        let account = provider.account_for(None).unwrap();
        let value = json!({
            "resultCode": "Authorised",
            "pspReference": PSP_FIXTURE,
            "merchantReference": "ORDER-1",
            "amount": { "value": 9900, "currency": "EUR" }
        });
        let flow = PaymentFlow::Redirect {
            success_url: "https://merchant.example.com/return".into(),
            cancel_url: None,
        };
        let automatic = provider
            .parse_payment_response(
                account,
                &value,
                "ORDER-1",
                None,
                &flow,
                Some(CaptureMode::Automatic),
            )
            .unwrap();
        assert_eq!(automatic.status, PaymentStatus::Succeeded);
        assert_eq!(automatic.provider_status, "Authorised");
        assert_eq!(automatic.provider_payment_id.as_deref(), Some(PSP_FIXTURE));
        assert_eq!(automatic.amount, Money::new(Currency::EUR, 9900));
        assert_eq!(
            automatic.amount_captured,
            Some(Money::new(Currency::EUR, 9900))
        );

        let manual = provider
            .parse_payment_response(
                account,
                &value,
                "ORDER-1",
                None,
                &flow,
                Some(CaptureMode::Manual),
            )
            .unwrap();
        assert_eq!(manual.status, PaymentStatus::Authorized);
        assert_eq!(manual.amount_captured, None);

        let queried = provider
            .parse_payment_response(account, &value, "ORDER-1", None, &flow, None)
            .unwrap();
        assert_eq!(queried.status, PaymentStatus::Authorized);

        assert!(provider
            .parse_payment_response(account, &json!({}), "ORDER-1", None, &flow, None)
            .is_err());
    }

    #[test]
    fn parses_modification_and_refund_responses() {
        let provider = test_provider();
        let account = provider.account_for(None).unwrap();
        let capture = json!({
            "merchantAccount": "FixtureMerchant",
            "paymentPspReference": PSP_FIXTURE,
            "pspReference": "JDD6LKT8MBLZNN84",
            "reference": "ORDER-1",
            "status": "received",
            "amount": { "value": 2500, "currency": "EUR" }
        });
        let intent = provider
            .parse_modification_payment(
                account,
                &capture,
                "ORDER-1",
                Some(&Money::new(Currency::EUR, 2500)),
            )
            .unwrap();
        assert_eq!(intent.status, PaymentStatus::Processing);
        assert_eq!(intent.provider_status, "received");
        assert_eq!(intent.provider_payment_id.as_deref(), Some(PSP_FIXTURE));

        let refund_request = RefundCommand::new(PSP_FIXTURE, "REFUND-1")
            .with_amount(Money::new(Currency::EUR, 2500));
        let refund = provider
            .parse_refund(account, &capture, &refund_request)
            .unwrap();
        assert_eq!(refund.status, webx_pay::RefundStatus::Pending);
        assert_eq!(
            refund.provider_refund_id.as_deref(),
            Some("JDD6LKT8MBLZNN84")
        );
        assert_eq!(refund.refund_id, "REFUND-1");
        assert_eq!(refund.amount, Money::new(Currency::EUR, 2500));
    }

    #[tokio::test]
    async fn create_payment_posts_the_documented_request() {
        let upstream = MockUpstream::start().await.unwrap();
        upstream.json(
            200,
            &json!({
                "resultCode": "Authorised",
                "pspReference": PSP_FIXTURE,
                "merchantReference": "ORDER-1",
                "amount": { "value": 9900, "currency": "EUR" }
            }),
        );
        let provider = AdyenProvider::new(mock_options(&upstream));
        let intent = provider
            .create_payment(&scope(), redirect_request())
            .await
            .unwrap();
        assert_eq!(intent.status, PaymentStatus::Succeeded);

        let requests = upstream.requests();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/v71/payments");
        let header = |name: &str| {
            request
                .headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(header("x-api-key").as_deref(), Some("fixture-api-key"));
        assert_eq!(header("content-type").as_deref(), Some("application/json"));
        assert_eq!(header("idempotency-key").as_deref(), Some("ORDER-1"));
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["reference"], json!("ORDER-1"));
        assert_eq!(body["amount"], json!({ "value": 9900, "currency": "EUR" }));
        assert_eq!(body["merchantAccount"], json!("FixtureMerchant"));
        assert_eq!(body["paymentMethod"], json!({ "type": "ideal" }));
        assert_eq!(body["channel"], json!("Web"));
        assert_eq!(
            body["returnUrl"],
            json!("https://merchant.example.com/return")
        );
        upstream.shutdown();
    }

    #[tokio::test]
    async fn captures_and_refunds_use_the_psp_reference_path() {
        let upstream = MockUpstream::start().await.unwrap();
        upstream.json(
            201,
            &json!({
                "merchantAccount": "FixtureMerchant",
                "paymentPspReference": PSP_FIXTURE,
                "pspReference": "JDD6LKT8MBLZNN84",
                "reference": PSP_FIXTURE,
                "status": "received",
                "amount": { "value": 2500, "currency": "EUR" }
            }),
        );
        upstream.json(
            201,
            &json!({
                "merchantAccount": "FixtureMerchant",
                "paymentPspReference": PSP_FIXTURE,
                "pspReference": "VK9DRSLLRCQ2WN82",
                "reference": "REFUND-1",
                "status": "received",
                "amount": { "value": 2500, "currency": "EUR" }
            }),
        );
        let provider = AdyenProvider::new(mock_options(&upstream));

        let captured = provider
            .capture_payment(&scope(), PSP_FIXTURE, Some(Money::new(Currency::EUR, 2500)))
            .await
            .unwrap();
        assert_eq!(captured.status, PaymentStatus::Processing);

        let refunded = provider
            .refund(
                &scope(),
                RefundCommand::new(PSP_FIXTURE, "REFUND-1")
                    .with_amount(Money::new(Currency::EUR, 2500)),
            )
            .await
            .unwrap();
        assert_eq!(refunded.status, webx_pay::RefundStatus::Pending);

        let requests = upstream.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].path,
            format!("/v71/payments/{PSP_FIXTURE}/captures")
        );
        assert_eq!(
            requests[1].path,
            format!("/v71/payments/{PSP_FIXTURE}/refunds")
        );
        let capture_body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(
            capture_body,
            json!({
                "merchantAccount": "FixtureMerchant",
                "amount": { "value": 2500, "currency": "EUR" },
                "reference": PSP_FIXTURE
            })
        );
        let refund_body: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
        assert_eq!(refund_body["reference"], json!("REFUND-1"));
        assert_eq!(
            refund_body["amount"],
            json!({ "value": 2500, "currency": "EUR" })
        );
        assert_eq!(
            requests[1]
                .headers
                .iter()
                .find(|(key, _)| key == "idempotency-key")
                .map(|(_, value)| value.as_str()),
            Some("REFUND-1")
        );
        upstream.shutdown();
    }

    #[tokio::test]
    async fn capture_without_an_amount_is_rejected_before_any_request() {
        let upstream = MockUpstream::start().await.unwrap();
        let provider = AdyenProvider::new(mock_options(&upstream));
        let error = provider
            .capture_payment(&scope(), PSP_FIXTURE, None)
            .await
            .unwrap_err();
        assert_eq!(error.http_status(), 400);
        assert!(error.public_detail().unwrap().contains("explicit amount"));
        assert!(upstream.requests().is_empty());
        upstream.shutdown();
    }

    #[tokio::test]
    async fn cancel_acknowledgements_carry_the_unknown_amount_placeholder() {
        let upstream = MockUpstream::start().await.unwrap();
        // Adyen's /cancels response has no amount field at all.
        upstream.json(
            201,
            &json!({
                "merchantAccount": "FixtureMerchant",
                "paymentPspReference": PSP_FIXTURE,
                "pspReference": "JDD6LKT8MBLZNN84",
                "reference": PSP_FIXTURE,
                "status": "received"
            }),
        );
        let provider = AdyenProvider::new(mock_options(&upstream));
        let canceled = provider
            .cancel_payment(&scope(), PSP_FIXTURE)
            .await
            .unwrap();
        assert_eq!(canceled.status, PaymentStatus::Processing);
        assert_eq!(canceled.provider_status, "received");
        assert_eq!(canceled.amount.minor, 0);
        assert_eq!(canceled.amount.currency.code(), "XXX");
        assert_eq!(canceled.provider_payment_id.as_deref(), Some(PSP_FIXTURE));
        assert_eq!(
            upstream.requests()[0].path,
            format!("/v71/payments/{PSP_FIXTURE}/cancels")
        );
        upstream.shutdown();
    }

    #[tokio::test]
    async fn refund_without_an_amount_is_rejected_before_any_request() {
        let upstream = MockUpstream::start().await.unwrap();
        let provider = AdyenProvider::new(mock_options(&upstream));
        let error = provider
            .refund(&scope(), RefundCommand::new(PSP_FIXTURE, "REFUND-1"))
            .await
            .unwrap_err();
        assert_eq!(error.http_status(), 400);
        assert!(error.public_detail().unwrap().contains("explicit amount"));
        assert!(upstream.requests().is_empty());
        upstream.shutdown();
    }

    #[tokio::test]
    async fn query_and_get_refund_are_unsupported_with_actionable_text() {
        let provider = test_provider();
        let query = provider.get_payment(&scope(), "ORDER-1").await.unwrap_err();
        assert_eq!(query.http_status(), 400);
        assert!(query
            .public_detail()
            .unwrap()
            .contains("get_payment_with_details"));

        let refund = provider.get_refund(&scope(), "REFUND-1").await.unwrap_err();
        assert_eq!(refund.http_status(), 400);
        assert!(refund.public_detail().unwrap().contains("REFUND"));

        let metadata = BTreeMap::new();
        let unsupported = provider
            .get_payment_with_metadata(None, "ORDER-1", &metadata)
            .await
            .unwrap_err();
        assert!(matches!(unsupported, PaymentError::Unsupported { .. }));
    }

    #[tokio::test]
    async fn get_payment_with_metadata_posts_payment_details() {
        let upstream = MockUpstream::start().await.unwrap();
        upstream.json(
            200,
            &json!({
                "resultCode": "Authorised",
                "pspReference": PSP_FIXTURE,
                "merchantReference": "ORDER-1",
                "amount": { "value": 9900, "currency": "EUR" }
            }),
        );
        let provider = AdyenProvider::new(mock_options(&upstream));
        let mut metadata = BTreeMap::new();
        metadata.insert(
            crate::METADATA_DETAILS.to_string(),
            r#"{"redirectResult":"fixture-redirect-result"}"#.to_string(),
        );
        metadata.insert(
            crate::METADATA_PAYMENT_DATA.to_string(),
            "fixture-payment-data".to_string(),
        );
        let intent = provider
            .get_payment_with_metadata(None, "ORDER-1", &metadata)
            .await
            .unwrap();
        assert_eq!(intent.status, PaymentStatus::Authorized);
        assert_eq!(intent.order_id, "ORDER-1");

        let requests = upstream.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/v71/payments/details");
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(
            body,
            json!({
                "details": { "redirectResult": "fixture-redirect-result" },
                "paymentData": "fixture-payment-data"
            })
        );
        upstream.shutdown();
    }

    #[tokio::test]
    async fn provider_errors_are_mapped_to_the_shared_error_model() {
        let upstream = MockUpstream::start().await.unwrap();
        upstream.json(
            422,
            &json!({
                "status": 422,
                "errorCode": "14_030",
                "message": "Return URL is missing."
            }),
        );
        let provider = AdyenProvider::new(mock_options(&upstream));
        let error = provider
            .create_payment(&scope(), redirect_request())
            .await
            .unwrap_err();
        assert_eq!(error.http_status(), 400);
        assert!(error.public_detail().unwrap().contains("14_030"));
        upstream.shutdown();
    }

    #[tokio::test]
    async fn provider_factory_resolves_from_the_options_monitor() {
        let monitor = Arc::new(webx_core::options_monitor::OptionsMonitor::new(
            test_options(),
        ));
        let factory = AdyenProviderFactory::new(monitor);
        assert_eq!(factory.provider(), PROVIDER);
        let provider = factory.resolve(&scope()).await.unwrap();
        assert_eq!(provider.id(), PROVIDER);
        assert_eq!(provider.accounts().len(), 1);
    }

    #[test]
    fn provider_builder_parses_settings_and_validates() {
        let settings = json!({
            "accounts": [{
                "api_key": "fixture-api-key",
                "merchant_account": "FixtureMerchant",
                "hmac_key": HMAC_FIXTURE
            }]
        });
        let config = PaymentProviderConfig::new(PROVIDER, settings);
        assert_eq!(AdyenProviderBuilder.provider(), PROVIDER);
        let provider = AdyenProviderBuilder.build(&config).unwrap();
        assert_eq!(provider.id(), PROVIDER);
        assert_eq!(provider.accounts().len(), 1);

        let invalid = AdyenProviderBuilder
            .build(&PaymentProviderConfig::new(PROVIDER, json!({})))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(invalid.http_status(), 500);
    }
}
