//! PayPal provider: Orders v2 operations and host registration.
//!
//! # Provider order ids
//!
//! PayPal Orders v2 has no lookup by merchant reference, so the methods that
//! take an `order_id` ([`get_payment`](IPaymentProvider::get_payment),
//! [`capture_payment`](IPaymentProvider::capture_payment),
//! [`cancel_payment`](IPaymentProvider::cancel_payment) and
//! [`refund`](IPaymentProvider::refund)) expect the **PayPal order id** that the
//! adapter returns as [`PaymentIntent::provider_payment_id`] (alphanumeric,
//! for example `5O190127TN364715T`). The merchant order id is stored in
//! `purchase_units[].custom_id` and echoed back by capture/refund webhooks.
//!
//! # Capture semantics
//!
//! * `intent=CAPTURE` orders (default) are captured with
//!   `POST /v2/checkout/orders/{id}/capture`; that endpoint has no amount field,
//!   so an amount override is accepted only when it equals the order amount.
//! * `intent=AUTHORIZE` orders (manual capture) are captured through
//!   `POST /v2/payments/authorizations/{id}/capture`, which supports partial
//!   amounts, after resolving the authorization id from the order.
//! * Only a manual-capture authorization can be cancelled (voided); a captured
//!   order cannot be cancelled and reports `Unsupported`.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use rust_dix::IServiceResolverExt;
use serde_json::Value;
use webx_client::IHttpClient;
use webx_host::health::HealthCheckSpec;
use webx_host::health::HealthStatus;
use webx_host::server::HostBuilder;
use webx_pay::{
    AccountId, AccountSet, ClientAction, CreatePayment, CurrencyScope, IPaymentProvider,
    IPaymentProviderBuilder, IPaymentProviderFactory, IPaymentWebhookVerifier, Money,
    PaymentAccount, PaymentError, PaymentFlow, PaymentFlowKind, PaymentIntent, PaymentMethod,
    PaymentOption, PaymentProviderConfig, PaymentScope, PaymentStatus,
    PaymentsServiceCollectionExt, ProviderCapabilities, ProviderId, Refund, RefundCommand,
    WebhookCapability,
};

use crate::auth::{PayPalClock, SystemClock};
use crate::mapping::map_capture_status;
use crate::model::{
    build_authorization_capture_body, build_create_order_body, build_order_capture_body,
    build_refund_body, client_sdk_action, order_to_intent, refund_to_refund, CaptureJson,
    PaypalOrder, RefundJson,
};
use crate::options::{PayPalAccount, PayPalOptions};
use crate::transport::{PayPalTransport, Verb};
use crate::webhook::PayPalWebhookVerifier;
use crate::PROVIDER;

/// Maximum length PayPal accepts for `PayPal-Request-Id` idempotency keys.
const MAX_REQUEST_ID_LENGTH: usize = 108;
/// Maximum length of a PayPal resource id used in a request path.
const MAX_RESOURCE_ID_LENGTH: usize = 64;

/// PayPal adapter.
pub struct PayPalProvider {
    accounts: Result<AccountSet<PayPalAccount>, String>,
    transport: Arc<PayPalTransport>,
    verifier: PayPalWebhookVerifier,
    validated: OnceLock<Result<(), String>>,
}

impl PayPalProvider {
    /// Build a provider from options (configuration is validated at startup).
    pub fn new(options: PayPalOptions) -> Self {
        Self::build(options, None, Arc::new(SystemClock))
    }

    /// Build a provider with an explicit HTTP client (tests, shared pool).
    pub fn with_client(options: PayPalOptions, client: Arc<dyn IHttpClient>) -> Self {
        Self::build(options, Some(client), Arc::new(SystemClock))
    }

    /// Build a provider with an injected clock for token-expiry testing.
    pub fn with_clock(options: PayPalOptions, clock: Arc<dyn PayPalClock>) -> Self {
        Self::build(options, None, clock)
    }

    fn build(
        options: PayPalOptions,
        injected_client: Option<Arc<dyn IHttpClient>>,
        clock: Arc<dyn PayPalClock>,
    ) -> Self {
        let accounts = options.build_accounts(clock);
        let transport = Arc::new(PayPalTransport::new(&options, injected_client));
        let verifier = PayPalWebhookVerifier::new(accounts.clone(), Arc::clone(&transport));
        Self {
            accounts,
            transport,
            verifier,
            validated: OnceLock::new(),
        }
    }

    fn account_set(&self) -> Result<&AccountSet<PayPalAccount>, PaymentError> {
        self.accounts
            .as_ref()
            .map_err(|error| PaymentError::config(PROVIDER, error.clone()))
    }

    fn account_for(
        &self,
        requested: Option<&AccountId>,
    ) -> Result<&Arc<PayPalAccount>, PaymentError> {
        self.account_set()?.resolve(requested)
    }

    /// Synchronous health probe: configuration parses and the HTTP client builds.
    pub fn health(&self) -> HealthStatus {
        match self.validate() {
            Ok(()) => HealthStatus::pass(),
            Err(error) => HealthStatus::fail(error.to_string()),
        }
    }

    async fn fetch_order(
        &self,
        account: &PayPalAccount,
        order_id: &str,
    ) -> Result<PaypalOrder, PaymentError> {
        let response = self
            .transport
            .send(
                account,
                Verb::Get,
                &format!("/v2/checkout/orders/{order_id}"),
                None,
                None,
                false,
            )
            .await?;
        PaypalOrder::parse(&response.value)
    }
}

#[async_trait]
impl IPaymentProvider for PayPalProvider {
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
            query: true,
            capture: true,
            cancel: true,
            refund: true,
            partial_refund: true,
            hosted_checkout: true,
            qr: false,
            app: false,
            webhook: WebhookCapability::ProviderApi,
            currencies: CurrencyScope::IsoAll,
            statuses: &[
                PaymentStatus::Created,
                PaymentStatus::RequiresAction,
                PaymentStatus::Processing,
                PaymentStatus::Authorized,
                PaymentStatus::Succeeded,
                PaymentStatus::Canceled,
                PaymentStatus::Failed,
                PaymentStatus::Refunded,
                PaymentStatus::PartiallyRefunded,
            ],
        }
    }

    /// One option per configured account for the checkout page.
    ///
    /// PayPal serves one method ([`PaymentMethod::PayPal`]) through the JS SDK
    /// or a hosted redirect, and the client configuration only carries the
    /// public `client_id` plus the `sandbox`/`live` environment. The client
    /// secret and webhook id never enter the catalog. Invalid configuration
    /// yields an empty list, matching [`Self::accounts`].
    fn payment_options(&self) -> Vec<PaymentOption> {
        let accounts = match &self.accounts {
            Ok(accounts) => accounts,
            Err(_) => return Vec::new(),
        };
        accounts
            .iter()
            .map(|(id, account)| {
                let mut option = PaymentOption::new(
                    PROVIDER,
                    id.clone(),
                    PaymentMethod::PayPal,
                    [PaymentFlowKind::ClientSdk, PaymentFlowKind::Redirect],
                )
                .with_client_config(client_config(account));
                if let Some(label) = account_label(accounts, id) {
                    option = option.with_label(label);
                }
                option
            })
            .collect()
    }

    fn validate(&self) -> Result<(), PaymentError> {
        let result = self.validated.get_or_init(|| {
            self.account_set().map_err(|error| error.to_string())?;
            self.transport
                .validate()
                .map_err(|error| error.to_string())?;
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
        if !matches!(request.method, PaymentMethod::PayPal) {
            return Err(PaymentError::unsupported(PROVIDER, "payment_method"));
        }
        match &request.flow {
            PaymentFlow::Redirect { .. } | PaymentFlow::ClientSdk { .. } => {}
            PaymentFlow::QrCode | PaymentFlow::App { .. } => {
                return Err(PaymentError::unsupported(PROVIDER, "flow"))
            }
        }
        let account = Arc::clone(self.account_for(scope.account_ref())?);
        let request_key = request_key(request.idempotency_key.as_deref(), &request.order_id)?;
        let body = build_create_order_body(&request, &request.flow);
        let response = self
            .transport
            .send(
                &account,
                Verb::Post,
                "/v2/checkout/orders",
                Some(&body),
                Some(&request_key),
                true,
            )
            .await?;
        let order = PaypalOrder::parse(&response.value)?;
        let mut intent = order_to_intent(
            &order,
            account.id.clone(),
            Some(&request.order_id),
            Some(request.amount),
            response.debug_id,
        )?;
        intent.client_action = match &request.flow {
            PaymentFlow::Redirect { .. } => {
                order.payer_action_link().map(|url| ClientAction::Redirect {
                    url: url.to_string(),
                })
            }
            _ => intent.provider_payment_id.as_deref().map(client_sdk_action),
        };
        Ok(intent)
    }

    async fn get_payment(
        &self,
        scope: &PaymentScope,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        let account = Arc::clone(self.account_for(scope.account_ref())?);
        let order_id = safe_resource_id(order_id)?;
        let order = self.fetch_order(&account, order_id).await?;
        let mut intent = order_to_intent(&order, account.id.clone(), Some(order_id), None, None)?;
        if intent.status == PaymentStatus::RequiresAction {
            intent.client_action = order.payer_action_link().map(|url| ClientAction::Redirect {
                url: url.to_string(),
            });
        }
        Ok(intent)
    }

    async fn capture_payment(
        &self,
        scope: &PaymentScope,
        order_id: &str,
        amount: Option<Money>,
    ) -> Result<PaymentIntent, PaymentError> {
        let account = Arc::clone(self.account_for(scope.account_ref())?);
        let order_id = safe_resource_id(order_id)?;
        let order = self.fetch_order(&account, order_id).await?;
        let order_amount = order.money();
        check_capture_amount(amount, order_amount)?;
        let request_key = request_key(None, &format!("{order_id}-capture"))?;

        if order.intent.as_deref() == Some("AUTHORIZE") {
            let authorization = order
                .first_authorization()
                .filter(|authorization| !authorization.id.is_empty())
                .ok_or_else(|| {
                    PaymentError::conflict(PROVIDER, "paypal order has no authorization to capture")
                })?;
            let authorization_id = safe_resource_id(&authorization.id)?;
            // Close the authorization when the whole amount is captured.
            let final_capture = amount.is_none_or(|amount| Some(amount) == order_amount);
            let body = build_authorization_capture_body(amount, final_capture);
            let response = self
                .transport
                .send(
                    &account,
                    Verb::Post,
                    &format!("/v2/payments/authorizations/{authorization_id}/capture"),
                    Some(&body),
                    Some(&request_key),
                    true,
                )
                .await?;
            let capture = CaptureJson::parse(&response.value)?;
            let mut intent = order_to_intent(
                &order,
                account.id.clone(),
                Some(order_id),
                None,
                response.debug_id,
            )?;
            intent.status = map_capture_status(&capture.status);
            intent.provider_status = capture.status.clone();
            intent.amount_captured = capture.money();
            intent.client_action = None;
            return Ok(intent);
        }

        if amount.is_some() && amount != order_amount {
            return Err(PaymentError::unsupported(PROVIDER, "capture.partial"));
        }
        let body = build_order_capture_body();
        let response = self
            .transport
            .send(
                &account,
                Verb::Post,
                &format!("/v2/checkout/orders/{order_id}/capture"),
                Some(&body),
                Some(&request_key),
                true,
            )
            .await?;
        let captured = PaypalOrder::parse(&response.value)?;
        let mut intent = order_to_intent(
            &captured,
            account.id.clone(),
            Some(order_id),
            order_amount,
            response.debug_id,
        )?;
        intent.client_action = None;
        Ok(intent)
    }

    async fn cancel_payment(
        &self,
        scope: &PaymentScope,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        let account = Arc::clone(self.account_for(scope.account_ref())?);
        let order_id = safe_resource_id(order_id)?;
        let order = self.fetch_order(&account, order_id).await?;

        if order.status == "VOIDED"
            || order
                .first_authorization()
                .is_some_and(|authorization| authorization.status == "VOIDED")
        {
            let mut intent =
                order_to_intent(&order, account.id.clone(), Some(order_id), None, None)?;
            intent.status = PaymentStatus::Canceled;
            intent.provider_status = "VOIDED".to_string();
            intent.client_action = None;
            return Ok(intent);
        }
        if order.intent.as_deref() != Some("AUTHORIZE") {
            // Orders v2 has no order-cancel endpoint: an automatic-capture order
            // is either still approvable or already captured.
            return Err(PaymentError::unsupported(PROVIDER, "cancel.captured"));
        }
        let authorization = match order.first_authorization() {
            Some(authorization) if !authorization.id.is_empty() => authorization,
            _ => return Err(PaymentError::unsupported(PROVIDER, "cancel.unapproved")),
        };
        if matches!(
            authorization.status.as_str(),
            "CAPTURED" | "PARTIALLY_CAPTURED"
        ) {
            return Err(PaymentError::unsupported(PROVIDER, "cancel.captured"));
        }
        let authorization_id = safe_resource_id(&authorization.id)?;
        let request_key = request_key(None, &format!("{order_id}-void"))?;
        let response = self
            .transport
            .send(
                &account,
                Verb::Post,
                &format!("/v2/payments/authorizations/{authorization_id}/void"),
                None,
                Some(&request_key),
                true,
            )
            .await?;
        let mut intent = order_to_intent(
            &order,
            account.id.clone(),
            Some(order_id),
            None,
            response.debug_id,
        )?;
        intent.status = PaymentStatus::Canceled;
        intent.provider_status = response
            .value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("VOIDED")
            .to_string();
        intent.amount_captured = None;
        intent.client_action = None;
        Ok(intent)
    }

    async fn refund(
        &self,
        scope: &PaymentScope,
        request: RefundCommand,
    ) -> Result<Refund, PaymentError> {
        request.validate()?;
        let request_key = request_key(request.idempotency_key.as_deref(), &request.refund_id)?;
        let account = Arc::clone(self.account_for(scope.account_ref())?);
        let order_id = safe_resource_id(&request.order_id)?;
        let order = self.fetch_order(&account, order_id).await?;
        if let (Some(amount), Some(order_amount)) = (request.amount, order.money()) {
            if amount.currency != order_amount.currency {
                return Err(PaymentError::validation(
                    "refund amount currency does not match the captured amount",
                ));
            }
        }
        let capture_id = order.refundable_capture_id().ok_or_else(|| {
            PaymentError::not_found(
                PROVIDER,
                format!("paypal order `{order_id}` has no captured payment to refund"),
            )
        })?;
        let capture_id = safe_resource_id(capture_id)?;
        let body = build_refund_body(&request);
        let response = self
            .transport
            .send(
                &account,
                Verb::Post,
                &format!("/v2/payments/captures/{capture_id}/refund"),
                Some(&body),
                Some(&request_key),
                true,
            )
            .await?;
        let refund = RefundJson::parse(&response.value)?;
        refund_to_refund(
            &refund,
            account.id.clone(),
            Some(&request.order_id),
            Some(&request.refund_id),
            response.debug_id,
        )
    }

    async fn get_refund(
        &self,
        scope: &PaymentScope,
        refund_id: &str,
    ) -> Result<Refund, PaymentError> {
        let account = Arc::clone(self.account_for(scope.account_ref())?);
        let refund_id = safe_resource_id(refund_id)?;
        let response = self
            .transport
            .send(
                &account,
                Verb::Get,
                &format!("/v2/payments/refunds/{refund_id}"),
                None,
                None,
                false,
            )
            .await?;
        let refund = RefundJson::parse(&response.value)?;
        refund_to_refund(&refund, account.id.clone(), None, None, response.debug_id)
    }

    fn webhook_verifier(&self) -> &dyn IPaymentWebhookVerifier {
        &self.verifier
    }
}

/// Public PayPal JS SDK configuration for one account.
///
/// Only the client id is public; the client secret never leaves the adapter.
/// `environment` follows the account's configured [`PayPalEnvironment`], not
/// the `api_base` override: the same credentials exist in both environments, so
/// a proxy override must not relabel the catalog.
///
/// [`PayPalEnvironment`]: crate::options::PayPalEnvironment
fn client_config(account: &PayPalAccount) -> Value {
    serde_json::json!({
        "client_id": account.client_id.as_str(),
        "environment": if account.live { "live" } else { "sandbox" },
    })
}

/// Display label configured for `id` in the secret-free account view.
fn account_label(accounts: &AccountSet<PayPalAccount>, id: &AccountId) -> Option<String> {
    accounts
        .accounts()
        .iter()
        .find(|info| &info.id == id)
        .and_then(|info| info.label.clone())
}

/// Validate a provider resource id used inside a request path.
fn safe_resource_id(value: &str) -> Result<&str, PaymentError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_RESOURCE_ID_LENGTH
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(PaymentError::validation(
            "paypal requires the PayPal resource id (alphanumeric) because Orders v2 has no lookup by merchant reference",
        ));
    }
    Ok(value)
}

/// Resolve the PayPal `PayPal-Request-Id` idempotency key.
fn request_key(explicit: Option<&str>, fallback: &str) -> Result<String, PaymentError> {
    let key = explicit.unwrap_or(fallback);
    if key.is_empty() || key.len() > MAX_REQUEST_ID_LENGTH {
        return Err(PaymentError::validation(format!(
            "paypal idempotency key must be 1..={MAX_REQUEST_ID_LENGTH} characters"
        )));
    }
    Ok(key.to_string())
}

fn check_capture_amount(
    amount: Option<Money>,
    order_amount: Option<Money>,
) -> Result<(), PaymentError> {
    let (Some(amount), Some(order_amount)) = (amount, order_amount) else {
        return Ok(());
    };
    if amount.currency != order_amount.currency {
        return Err(PaymentError::validation(
            "capture amount currency does not match the authorized amount",
        ));
    }
    if amount.minor > order_amount.minor {
        return Err(PaymentError::validation(
            "capture amount exceeds the authorized amount",
        ));
    }
    Ok(())
}

/// Register PayPal on a [`HostBuilder`].
pub trait PayPalHostBuilderExt: Sized {
    /// Register the adapter with runtime configuration.
    ///
    /// Binds the `"PayPal"` configuration section through
    /// `add_options`, so appsettings/file reloads are picked up by the
    /// runtime cache (default 60s) or immediately after
    /// `Payments::invalidate`.
    fn add_paypal(self) -> Self;

    /// Register the adapter with a prebuilt static configuration.
    fn add_paypal_with(self, options: PayPalOptions) -> Self;
}

impl PayPalHostBuilderExt for HostBuilder {
    fn add_paypal(self) -> Self {
        let builder = self.add_options::<PayPalOptions>("PayPal");
        builder.register(|services| {
            services
                .add_payment_provider_factory_lazy(|resolver| {
                    let monitor = resolver
                        .resolve_arc::<dyn webx_core::options_monitor::IOptionsMonitor<PayPalOptions>>()
                        .expect("add_paypal() registers the PayPal options monitor");
                    Arc::new(PayPalProviderFactory::new(monitor))
                })
                .add_payment_provider_builder(Arc::new(PayPalProviderBuilder))
        })
    }

    fn add_paypal_with(self, options: PayPalOptions) -> Self {
        let provider = Arc::new(PayPalProvider::new(options));
        let health = Arc::clone(&provider);
        self.register(move |services| {
            services
                .add_payment_provider(provider)
                .add_payment_provider_builder(Arc::new(PayPalProviderBuilder))
        })
        .add_health_check(HealthCheckSpec::sync("payments.paypal", move || {
            health.health()
        }))
    }
}

/// Runtime provider factory for PayPal (reads the options monitor).
pub struct PayPalProviderFactory {
    monitor: Arc<dyn webx_core::options_monitor::IOptionsMonitor<PayPalOptions>>,
}

impl PayPalProviderFactory {
    /// Build the factory over an options monitor.
    pub fn new(
        monitor: Arc<dyn webx_core::options_monitor::IOptionsMonitor<PayPalOptions>>,
    ) -> Self {
        Self { monitor }
    }
}

#[async_trait]
impl IPaymentProviderFactory for PayPalProviderFactory {
    fn provider(&self) -> ProviderId {
        PROVIDER
    }

    async fn resolve(
        &self,
        _scope: &PaymentScope,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        let options = self.monitor.current();
        let provider = PayPalProvider::new((*options).clone());
        provider.validate()?;
        Ok(Arc::new(provider))
    }
}

/// Builder used by custom `IPaymentServiceFactory` implementations.
#[derive(Debug, Default, Clone, Copy)]
pub struct PayPalProviderBuilder;

impl IPaymentProviderBuilder for PayPalProviderBuilder {
    fn provider(&self) -> ProviderId {
        PROVIDER
    }

    fn build(
        &self,
        config: &PaymentProviderConfig,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        let options: PayPalOptions =
            serde_json::from_value(config.settings.clone()).map_err(|error| {
                PaymentError::config(
                    PROVIDER,
                    format!("invalid PayPal provider settings: {error}"),
                )
            })?;
        let provider = PayPalProvider::new(options);
        provider.validate()?;
        Ok(Arc::new(provider))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use webx_client::testing::{MockUpstream, RecordedRequest};
    use webx_pay::{Currency, PaymentEventType};

    use super::*;
    use crate::auth::basic_auth_header;
    use crate::options::{PayPalAccountOptions, PayPalEnvironment};

    fn provider_for(upstream: &MockUpstream) -> PayPalProvider {
        PayPalProvider::new(
            PayPalOptions::new("test-client", "test-secret", "test-webhook")
                .with_api_base(upstream.base_url())
                .allow_private_networks(true)
                .with_timeout(Duration::from_secs(5)),
        )
    }

    fn scope() -> PaymentScope {
        PaymentScope::new(ProviderId::PayPal)
    }

    fn header<'a>(request: &'a RecordedRequest, name: &str) -> Option<&'a str> {
        request
            .headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn body_json(request: &RecordedRequest) -> Value {
        serde_json::from_slice(&request.body).expect("request body should be JSON")
    }

    fn token_response() -> Value {
        serde_json::json!({
            "scope": "https://uri.paypal.com/services/payments",
            "access_token": "A21AA-test-token",
            "token_type": "Bearer",
            "app_id": "APP-1",
            "expires_in": 32400
        })
    }

    fn order_fixture() -> Value {
        serde_json::json!({
            "id": "5O190127TN364715T",
            "status": "PAYER_ACTION_REQUIRED",
            "intent": "CAPTURE",
            "create_time": "2024-10-14T21:29:26Z",
            "purchase_units": [{
                "reference_id": "ORDER-1",
                "custom_id": "ORDER-1",
                "amount": { "currency_code": "USD", "value": "99.00" }
            }],
            "links": [
                {
                    "href": "https://api-m.paypal.com/v2/checkout/orders/5O190127TN364715T",
                    "rel": "self"
                },
                {
                    "href": "https://www.paypal.com/checkoutnow?token=5O190127TN364715T",
                    "rel": "payer-action"
                }
            ]
        })
    }

    #[test]
    fn rejects_merchant_order_ids_and_path_characters() {
        assert_eq!(
            safe_resource_id("5O190127TN364715T").unwrap(),
            "5O190127TN364715T"
        );
        assert!(safe_resource_id("ORDER-1").is_err());
        assert!(safe_resource_id("../orders").is_err());
        assert!(safe_resource_id("").is_err());
        assert!(safe_resource_id(&"A".repeat(65)).is_err());
    }

    #[test]
    fn idempotency_keys_are_bounded() {
        assert_eq!(request_key(None, "ORDER-1").unwrap(), "ORDER-1");
        assert_eq!(
            request_key(Some("explicit-key"), "ignored").unwrap(),
            "explicit-key"
        );
        assert!(request_key(None, "").is_err());
        assert!(request_key(None, &"k".repeat(109)).is_err());
    }

    #[test]
    fn capture_amount_must_match_currency_and_not_exceed_authorization() {
        assert!(check_capture_amount(None, Some(Money::usd(100))).is_ok());
        assert!(check_capture_amount(Some(Money::usd(100)), Some(Money::usd(100))).is_ok());
        assert!(check_capture_amount(Some(Money::usd(101)), Some(Money::usd(100))).is_err());
        assert!(check_capture_amount(Some(Money::usd(50)), Some(Money::usd(100))).is_ok());
        assert!(check_capture_amount(Some(Money::cny(100)), Some(Money::usd(100))).is_err());
    }

    #[test]
    fn capabilities_declare_provider_api_webhooks() {
        let provider = PayPalProvider::new(PayPalOptions::new("test-client", "test-secret", ""));
        let capabilities = provider.capabilities();
        assert!(capabilities.create);
        assert!(capabilities.query);
        assert!(capabilities.capture);
        assert!(capabilities.cancel);
        assert!(capabilities.refund);
        assert!(capabilities.partial_refund);
        assert_eq!(capabilities.webhook, WebhookCapability::ProviderApi);
        assert_eq!(provider.id(), ProviderId::PayPal);
    }

    #[test]
    fn health_and_validate_report_missing_configuration() {
        let missing = PayPalProvider::new(PayPalOptions::empty());
        assert!(missing.validate().is_err());
        assert_eq!(missing.health().status, "fail");

        let configured = PayPalProvider::new(PayPalOptions::new(
            "test-client",
            "test-secret",
            "test-webhook",
        ));
        assert!(configured.validate().is_ok());
        assert_eq!(configured.health().status, "pass");
    }

    #[test]
    fn payment_options_publish_public_client_config_per_account() {
        let provider = PayPalProvider::new(PayPalOptions::new(
            "public-client-id",
            "super-secret-value",
            "test-webhook",
        ));
        let options = provider.payment_options();
        assert_eq!(options.len(), 1);
        let option = &options[0];
        assert_eq!(option.provider, ProviderId::PayPal);
        assert_eq!(option.account, AccountId::default_account());
        assert_eq!(option.method, PaymentMethod::PayPal);
        assert_eq!(
            option.flows,
            vec![PaymentFlowKind::ClientSdk, PaymentFlowKind::Redirect]
        );
        assert_eq!(option.label, None);
        let config = option.client_config.as_ref().expect("client config");
        assert_eq!(config["client_id"], "public-client-id");
        assert_eq!(config["environment"], "sandbox");
        assert!(config.get("client_secret").is_none());

        let serialized = serde_json::to_string(&options).expect("catalog should serialize");
        assert!(!serialized.contains("super-secret-value"));
        assert!(!serialized.contains("test-webhook"));
        assert!(!serialized.contains("client_secret"));
    }

    #[test]
    fn payment_options_cover_every_account_and_follow_the_environment() {
        let options = PayPalOptions::empty()
            .with_account(PayPalAccountOptions::new(
                "sandbox-client",
                "sandbox-secret",
            ))
            .with_account(
                PayPalAccountOptions::new("live-client", "live-secret")
                    .with_id("live-eu")
                    .with_environment(PayPalEnvironment::Live)
                    .with_api_base("https://paypal-proxy.example.com"),
            )
            .with_default_account("live-eu");
        let provider = PayPalProvider::new(options);
        let catalog = provider.payment_options();
        assert_eq!(catalog.len(), 2);

        let sandbox = catalog
            .iter()
            .find(|option| option.account == AccountId::from("default"))
            .expect("sandbox option");
        let sandbox_config = sandbox.client_config.as_ref().expect("sandbox config");
        assert_eq!(sandbox_config["client_id"], "sandbox-client");
        assert_eq!(sandbox_config["environment"], "sandbox");
        assert_eq!(sandbox.method, PaymentMethod::PayPal);

        let live = catalog
            .iter()
            .find(|option| option.account == AccountId::from("live-eu"))
            .expect("live option");
        let live_config = live.client_config.as_ref().expect("live config");
        assert_eq!(live_config["client_id"], "live-client");
        // The api_base proxy override does not change the PayPal environment.
        assert_eq!(live_config["environment"], "live");
        assert_eq!(live.method, PaymentMethod::PayPal);
        assert_eq!(
            live.flows,
            vec![PaymentFlowKind::ClientSdk, PaymentFlowKind::Redirect]
        );
    }

    #[test]
    fn payment_options_are_empty_when_the_configuration_is_invalid() {
        let provider = PayPalProvider::new(PayPalOptions::empty());
        assert!(provider.payment_options().is_empty());

        let invalid = PayPalOptions::empty().with_account(PayPalAccountOptions {
            client_id: "  ".to_string(),
            ..PayPalAccountOptions::new("client", "secret")
        });
        assert!(PayPalProvider::new(invalid).payment_options().is_empty());
    }

    #[test]
    fn payment_options_use_the_public_account_label() {
        let mut provider = PayPalProvider::new(PayPalOptions::new("client", "secret", ""));
        let (id, account) = {
            let accounts = provider.account_set().expect("valid accounts");
            let (id, account) = accounts.iter().next().expect("one account");
            (id.clone(), (**account).clone())
        };
        let labelled = AccountSet::new(
            ProviderId::PayPal,
            id.clone(),
            [(
                id.clone(),
                account,
                PaymentAccount {
                    id: id.clone(),
                    provider: ProviderId::PayPal,
                    live: false,
                    label: Some("PayPal EU".to_string()),
                },
            )],
        )
        .expect("labelled account set");
        provider.accounts = Ok(labelled);

        let options = provider.payment_options();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].label.as_deref(), Some("PayPal EU"));
        assert_eq!(options[0].account, id);
    }

    #[tokio::test]
    async fn runtime_factory_resolves_from_the_options_monitor() {
        let monitor = Arc::new(webx_core::options_monitor::OptionsMonitor::new(
            PayPalOptions::new("test-client", "test-secret", "test-webhook"),
        ));
        let factory = PayPalProviderFactory::new(monitor);
        assert_eq!(factory.provider(), ProviderId::PayPal);
        let provider = factory.resolve(&scope()).await.unwrap();
        assert_eq!(provider.id(), ProviderId::PayPal);
        assert!(provider.validate().is_ok());
    }

    #[tokio::test]
    async fn builder_parses_settings_and_validates_them() {
        let builder = PayPalProviderBuilder;
        assert_eq!(builder.provider(), ProviderId::PayPal);

        let config = PaymentProviderConfig::new(
            ProviderId::PayPal,
            serde_json::json!({
                "accounts": [{
                    "client_id": "test-client",
                    "client_secret": "test-secret",
                    "webhook_id": "test-webhook"
                }]
            }),
        );
        let provider = builder.build(&config).unwrap();
        assert_eq!(provider.id(), ProviderId::PayPal);
        assert!(provider.validate().is_ok());

        let missing_accounts =
            PaymentProviderConfig::new(ProviderId::PayPal, serde_json::json!({ "accounts": [] }));
        assert!(matches!(
            builder.build(&missing_accounts),
            Err(PaymentError::Config { .. })
        ));

        let malformed =
            PaymentProviderConfig::new(ProviderId::PayPal, serde_json::json!("not settings"));
        assert!(matches!(
            builder.build(&malformed),
            Err(PaymentError::Config { .. })
        ));
    }

    #[tokio::test]
    async fn creates_a_redirect_order_with_oauth_and_idempotency() {
        let upstream = MockUpstream::start().await.unwrap();
        upstream.json(200, &token_response());
        upstream.json(201, &order_fixture());
        let provider = provider_for(&upstream);

        let request = CreatePayment::new(
            "ORDER-1",
            Money::usd(9900),
            PaymentMethod::PayPal,
            PaymentFlow::Redirect {
                success_url: "https://example.com/ok".to_string(),
                cancel_url: Some("https://example.com/cancel".to_string()),
            },
        );
        let intent = provider.create_payment(&scope(), request).await.unwrap();
        assert_eq!(intent.provider, ProviderId::PayPal);
        assert_eq!(intent.order_id, "ORDER-1");
        assert_eq!(
            intent.provider_payment_id.as_deref(),
            Some("5O190127TN364715T")
        );
        assert_eq!(intent.status, PaymentStatus::RequiresAction);
        assert_eq!(intent.amount, Money::usd(9900));
        assert_eq!(
            intent.client_action,
            Some(ClientAction::Redirect {
                url: "https://www.paypal.com/checkoutnow?token=5O190127TN364715T".to_string()
            })
        );

        let requests = upstream.requests();
        assert_eq!(requests.len(), 2);

        let token_request = &requests[0];
        assert_eq!(token_request.method, "POST");
        assert_eq!(token_request.path, "/v1/oauth2/token");
        assert_eq!(
            header(token_request, "authorization"),
            Some(basic_auth_header("test-client", "test-secret").as_str())
        );
        assert_eq!(
            header(token_request, "content-type"),
            Some("application/x-www-form-urlencoded")
        );
        assert_eq!(
            String::from_utf8_lossy(&token_request.body),
            "grant_type=client_credentials"
        );

        let order_request = &requests[1];
        assert_eq!(order_request.method, "POST");
        assert_eq!(order_request.path, "/v2/checkout/orders");
        assert_eq!(
            header(order_request, "authorization"),
            Some("Bearer A21AA-test-token")
        );
        assert_eq!(header(order_request, "paypal-request-id"), Some("ORDER-1"));
        assert_eq!(
            header(order_request, "prefer"),
            Some("return=representation")
        );
        let body = body_json(order_request);
        assert_eq!(body["intent"], "CAPTURE");
        assert_eq!(
            body["purchase_units"][0]["amount"],
            serde_json::json!({ "currency_code": "USD", "value": "99.00" })
        );
        assert_eq!(
            body["payment_source"]["paypal"]["experience_context"]["return_url"],
            "https://example.com/ok"
        );
    }

    #[tokio::test]
    async fn client_sdk_flow_returns_an_order_payload_and_reuses_the_token() {
        let upstream = MockUpstream::start().await.unwrap();
        upstream.json(200, &token_response());
        upstream.json(201, &order_fixture());
        let provider = provider_for(&upstream);

        let request = CreatePayment::new(
            "ORDER-1",
            Money::usd(9900),
            PaymentMethod::PayPal,
            PaymentFlow::ClientSdk { return_url: None },
        );
        let intent = provider.create_payment(&scope(), request).await.unwrap();
        assert_eq!(
            intent.client_action,
            Some(client_sdk_action("5O190127TN364715T"))
        );

        upstream.json(200, &order_fixture());
        let fetched = provider
            .get_payment(&scope(), "5O190127TN364715T")
            .await
            .unwrap();
        assert_eq!(fetched.status, PaymentStatus::RequiresAction);

        let requests = upstream.requests();
        assert_eq!(requests.len(), 3);
        // One token fetch, reused by the second call.
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path == "/v1/oauth2/token")
                .count(),
            1
        );
        assert_eq!(requests[2].method, "GET");
        assert_eq!(requests[2].path, "/v2/checkout/orders/5O190127TN364715T");
        assert_eq!(header(&requests[2], "paypal-request-id"), None);
    }

    #[tokio::test]
    async fn captures_an_authorized_order_through_the_authorization_endpoint() {
        let upstream = MockUpstream::start().await.unwrap();
        upstream.json(200, &token_response());
        upstream.json(
            200,
            &serde_json::json!({
                "id": "5O190127TN364715T",
                "status": "APPROVED",
                "intent": "AUTHORIZE",
                "create_time": "2024-10-14T21:29:26Z",
                "purchase_units": [{
                    "reference_id": "ORDER-1",
                    "custom_id": "ORDER-1",
                    "amount": { "currency_code": "USD", "value": "99.00" },
                    "payments": {
                        "authorizations": [{
                            "id": "7X550106NA4440028",
                            "status": "CREATED",
                            "amount": { "currency_code": "USD", "value": "99.00" }
                        }]
                    }
                }],
                "links": [{ "href": "https://api-m.paypal.com/x", "rel": "self" }]
            }),
        );
        upstream.json(
            201,
            &serde_json::json!({
                "id": "3C679366HH908993F",
                "status": "COMPLETED",
                "amount": { "currency_code": "USD", "value": "99.00" },
                "final_capture": true,
                "create_time": "2024-10-14T21:30:00Z"
            }),
        );
        let provider = provider_for(&upstream);

        let intent = provider
            .capture_payment(&scope(), "5O190127TN364715T", None)
            .await
            .unwrap();
        assert_eq!(intent.status, PaymentStatus::Succeeded);
        assert_eq!(intent.provider_status, "COMPLETED");
        assert_eq!(intent.amount_captured, Some(Money::usd(9900)));
        assert_eq!(intent.client_action, None);

        let requests = upstream.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[1].method, "GET");
        assert_eq!(requests[1].path, "/v2/checkout/orders/5O190127TN364715T");
        assert_eq!(requests[2].method, "POST");
        assert_eq!(
            requests[2].path,
            "/v2/payments/authorizations/7X550106NA4440028/capture"
        );
        assert_eq!(
            header(&requests[2], "paypal-request-id"),
            Some("5O190127TN364715T-capture")
        );
        let body = body_json(&requests[2]);
        assert_eq!(body, serde_json::json!({ "final_capture": true }));
    }

    #[tokio::test]
    async fn voids_an_authorization_when_cancelling_a_manual_capture_order() {
        let upstream = MockUpstream::start().await.unwrap();
        upstream.json(200, &token_response());
        upstream.json(
            200,
            &serde_json::json!({
                "id": "5O190127TN364715T",
                "status": "APPROVED",
                "intent": "AUTHORIZE",
                "purchase_units": [{
                    "custom_id": "ORDER-1",
                    "amount": { "currency_code": "USD", "value": "99.00" },
                    "payments": {
                        "authorizations": [{
                            "id": "7X550106NA4440028",
                            "status": "CREATED",
                            "amount": { "currency_code": "USD", "value": "99.00" }
                        }]
                    }
                }]
            }),
        );
        upstream.json(
            200,
            &serde_json::json!({
                "id": "7X550106NA4440028",
                "status": "VOIDED",
                "amount": { "currency_code": "USD", "value": "99.00" },
                "create_time": "2024-10-10T10:06:03-07:00",
                "update_time": "2024-10-10T10:06:19-07:00"
            }),
        );
        let provider = provider_for(&upstream);

        let intent = provider
            .cancel_payment(&scope(), "5O190127TN364715T")
            .await
            .unwrap();
        assert_eq!(intent.status, PaymentStatus::Canceled);
        assert_eq!(intent.provider_status, "VOIDED");

        let requests = upstream.requests();
        assert_eq!(requests[2].method, "POST");
        assert_eq!(
            requests[2].path,
            "/v2/payments/authorizations/7X550106NA4440028/void"
        );
        assert_eq!(
            header(&requests[2], "paypal-request-id"),
            Some("5O190127TN364715T-void")
        );
    }

    #[tokio::test]
    async fn captures_a_capture_intent_order_with_the_order_endpoint() {
        let upstream = MockUpstream::start().await.unwrap();
        upstream.json(200, &token_response());
        let approved_order = serde_json::json!({
            "id": "5O190127TN364715T",
            "status": "APPROVED",
            "intent": "CAPTURE",
            "create_time": "2024-10-14T21:29:26Z",
            "purchase_units": [{
                "reference_id": "ORDER-1",
                "custom_id": "ORDER-1",
                "amount": { "currency_code": "USD", "value": "99.00" }
            }],
            "links": [{ "href": "https://api-m.paypal.com/x", "rel": "self" }]
        });
        upstream.json(200, &approved_order);
        // One GET per capture attempt: the rejected partial call and the full
        // call both resolve the order first.
        upstream.json(200, &approved_order);
        upstream.json(
            201,
            &serde_json::json!({
                "id": "5O190127TN364715T",
                "status": "COMPLETED",
                "intent": "CAPTURE",
                "purchase_units": [{
                    "custom_id": "ORDER-1",
                    "amount": { "currency_code": "USD", "value": "99.00" },
                    "payments": { "captures": [{
                        "id": "3C679366HH908993F",
                        "status": "COMPLETED",
                        "amount": { "currency_code": "USD", "value": "99.00" }
                    }]}
                }],
                "links": [{ "href": "https://api-m.paypal.com/x", "rel": "self" }]
            }),
        );
        let provider = provider_for(&upstream);

        // Partial capture of an automatic-capture order is not supported by the
        // Orders v2 capture endpoint.
        let partial = provider
            .capture_payment(&scope(), "5O190127TN364715T", Some(Money::usd(500)))
            .await
            .unwrap_err();
        assert!(matches!(partial, PaymentError::Unsupported { .. }));

        let intent = provider
            .capture_payment(&scope(), "5O190127TN364715T", Some(Money::usd(9900)))
            .await
            .unwrap();
        assert_eq!(intent.status, PaymentStatus::Succeeded);
        assert_eq!(intent.amount_captured, Some(Money::usd(9900)));

        let requests = upstream.requests();
        assert_eq!(requests.len(), 4);
        let capture_request = &requests[3];
        assert_eq!(capture_request.method, "POST");
        assert_eq!(
            capture_request.path,
            "/v2/checkout/orders/5O190127TN364715T/capture"
        );
        assert_eq!(
            header(capture_request, "paypal-request-id"),
            Some("5O190127TN364715T-capture")
        );
        assert_eq!(body_json(capture_request), serde_json::json!({}));
    }

    #[tokio::test]
    async fn refuses_to_cancel_captured_orders() {
        let upstream = MockUpstream::start().await.unwrap();
        upstream.json(200, &token_response());
        upstream.json(
            200,
            &serde_json::json!({
                "id": "5O190127TN364715T",
                "status": "COMPLETED",
                "intent": "CAPTURE",
                "purchase_units": [{
                    "custom_id": "ORDER-1",
                    "amount": { "currency_code": "USD", "value": "99.00" },
                    "payments": { "captures": [{ "id": "CAP1", "status": "COMPLETED" }] }
                }]
            }),
        );
        let provider = provider_for(&upstream);
        let error = provider
            .cancel_payment(&scope(), "5O190127TN364715T")
            .await
            .unwrap_err();
        assert!(matches!(error, PaymentError::Unsupported { .. }));
    }

    #[tokio::test]
    async fn refunds_a_capture_with_the_merchant_refund_id() {
        let upstream = MockUpstream::start().await.unwrap();
        upstream.json(200, &token_response());
        upstream.json(
            200,
            &serde_json::json!({
                "id": "5O190127TN364715T",
                "status": "COMPLETED",
                "intent": "CAPTURE",
                "purchase_units": [{
                    "custom_id": "ORDER-1",
                    "amount": { "currency_code": "USD", "value": "99.00" },
                    "payments": { "captures": [{
                        "id": "3C679366HH908993F",
                        "status": "COMPLETED",
                        "amount": { "currency_code": "USD", "value": "99.00" }
                    }] }
                }]
            }),
        );
        upstream.json(
            201,
            &serde_json::json!({
                "id": "0K35355239430361V",
                "status": "COMPLETED",
                "amount": { "currency_code": "USD", "value": "10.00" },
                "custom_id": "ORDER-1",
                "invoice_id": "REFUND-1",
                "create_time": "2024-10-14T14:58:34-07:00",
                "update_time": "2024-10-14T14:58:34-07:00"
            }),
        );
        let provider = provider_for(&upstream);

        let refund = provider
            .refund(
                &scope(),
                RefundCommand::new("5O190127TN364715T", "REFUND-1").with_amount(Money::usd(1000)),
            )
            .await
            .unwrap();
        assert_eq!(refund.refund_id, "REFUND-1");
        assert_eq!(refund.order_id, "5O190127TN364715T");
        assert_eq!(
            refund.provider_refund_id.as_deref(),
            Some("0K35355239430361V")
        );
        assert_eq!(refund.amount, Money::usd(1000));

        let requests = upstream.requests();
        assert_eq!(
            requests[2].path,
            "/v2/payments/captures/3C679366HH908993F/refund"
        );
        assert_eq!(header(&requests[2], "paypal-request-id"), Some("REFUND-1"));
        let body = body_json(&requests[2]);
        assert_eq!(body["invoice_id"], "REFUND-1");
        assert_eq!(body["custom_id"], "5O190127TN364715T");
        assert_eq!(
            body["amount"],
            serde_json::json!({ "currency_code": "USD", "value": "10.00" })
        );

        upstream.json(
            200,
            &serde_json::json!({
                "id": "0K35355239430361V",
                "status": "COMPLETED",
                "amount": { "currency_code": "USD", "value": "10.00" },
                "custom_id": "5O190127TN364715T",
                "invoice_id": "REFUND-1",
                "create_time": "2024-10-14T14:58:34-07:00",
                "update_time": "2024-10-14T14:58:34-07:00"
            }),
        );
        let fetched = provider
            .get_refund(&scope(), "0K35355239430361V")
            .await
            .unwrap();
        assert_eq!(fetched.refund_id, "REFUND-1");
        assert_eq!(fetched.order_id, "5O190127TN364715T");
        assert_eq!(fetched.status, webx_pay::RefundStatus::Succeeded);
        let requests = upstream.requests();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[3].method, "GET");
        assert_eq!(requests[3].path, "/v2/payments/refunds/0K35355239430361V");
    }

    #[tokio::test]
    async fn maps_a_422_order_error_to_a_validation_failure() {
        let upstream = MockUpstream::start().await.unwrap();
        upstream.json(200, &token_response());
        upstream.json(
            422,
            &serde_json::json!({
                "name": "UNPROCESSABLE_ENTITY",
                "message": "The requested action could not be performed.",
                "debug_id": "debug-422",
                "details": [{ "issue": "ORDER_ALREADY_CAPTURED" }]
            }),
        );
        let provider = provider_for(&upstream);
        let request = CreatePayment::new(
            "ORDER-1",
            Money::usd(9900),
            PaymentMethod::PayPal,
            PaymentFlow::ClientSdk { return_url: None },
        );
        let error = provider
            .create_payment(&scope(), request)
            .await
            .unwrap_err();
        assert!(matches!(error, PaymentError::Validation { .. }));
        assert_eq!(error.http_status(), 400);
    }

    #[tokio::test]
    async fn webhook_verification_round_trips_through_paypal() {
        let upstream = MockUpstream::start().await.unwrap();
        upstream.json(200, &token_response());
        upstream.json(
            200,
            &serde_json::json!({ "verification_status": "SUCCESS" }),
        );
        let provider = provider_for(&upstream);

        let event = serde_json::json!({
            "id": "WH-2WE669878H127391W-3YM43211FL417032U",
            "event_type": "PAYMENT.CAPTURE.COMPLETED",
            "create_time": "2023-06-25T21:41:28Z",
            "resource_type": "capture",
            "resource": {
                "id": "3C679366HH908993F",
                "status": "COMPLETED",
                "custom_id": "ORDER-1",
                "amount": { "currency_code": "USD", "value": "99.00" }
            }
        });
        let payload = webx_pay::WebhookPayload::new(
            [
                (
                    "PAYPAL-TRANSMISSION-ID".to_string(),
                    "69cd13f0-d67a-11e5-baa3-778b53f4ae55".to_string(),
                ),
                (
                    "PAYPAL-TRANSMISSION-TIME".to_string(),
                    "2016-02-18T20:01:35Z".to_string(),
                ),
                (
                    "PAYPAL-TRANSMISSION-SIG".to_string(),
                    "signature-value".to_string(),
                ),
                (
                    "PAYPAL-CERT-URL".to_string(),
                    "https://api.paypal.com/cert.pem".to_string(),
                ),
                ("PAYPAL-AUTH-ALGO".to_string(), "SHA256withRSA".to_string()),
            ],
            serde_json::to_vec(&event).unwrap(),
        );

        let normalized = provider
            .webhook_verifier()
            .verify(&scope(), &payload)
            .await
            .unwrap();
        assert_eq!(normalized.event_type, PaymentEventType::Succeeded);
        assert_eq!(
            normalized.dedupe_key,
            "WH-2WE669878H127391W-3YM43211FL417032U"
        );
        assert_eq!(normalized.order_id.as_deref(), Some("ORDER-1"));

        let requests = upstream.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].method, "POST");
        assert_eq!(
            requests[1].path,
            "/v1/notifications/verify-webhook-signature"
        );
        assert_eq!(header(&requests[1], "paypal-request-id"), None);
        let body = body_json(&requests[1]);
        assert_eq!(body["webhook_id"], "test-webhook");
        assert_eq!(body["auth_algo"], "SHA256withRSA");
        assert_eq!(
            body["transmission_id"],
            "69cd13f0-d67a-11e5-baa3-778b53f4ae55"
        );
        assert_eq!(
            body["webhook_event"]["id"],
            "WH-2WE669878H127391W-3YM43211FL417032U"
        );
    }

    #[tokio::test]
    async fn webhook_verification_rejects_a_provider_failure() {
        let upstream = MockUpstream::start().await.unwrap();
        upstream.json(200, &token_response());
        upstream.json(
            200,
            &serde_json::json!({ "verification_status": "FAILURE" }),
        );
        let provider = provider_for(&upstream);
        let payload = webx_pay::WebhookPayload::new(
            [
                ("PAYPAL-TRANSMISSION-ID".to_string(), "id".to_string()),
                (
                    "PAYPAL-TRANSMISSION-TIME".to_string(),
                    "2016-02-18T20:01:35Z".to_string(),
                ),
                ("PAYPAL-TRANSMISSION-SIG".to_string(), "sig".to_string()),
                (
                    "PAYPAL-CERT-URL".to_string(),
                    "https://api.paypal.com/cert.pem".to_string(),
                ),
                ("PAYPAL-AUTH-ALGO".to_string(), "SHA256withRSA".to_string()),
            ],
            br#"{"id":"WH-1","event_type":"PAYMENT.CAPTURE.COMPLETED","resource":{}}"#.to_vec(),
        );
        let error = provider
            .webhook_verifier()
            .verify(&scope(), &payload)
            .await
            .unwrap_err();
        assert!(matches!(error, PaymentError::SignatureInvalid { .. }));
    }

    #[test]
    fn currency_scope_is_all_iso_codes() {
        let provider = PayPalProvider::new(PayPalOptions::new("test-client", "test-secret", ""));
        assert!(provider.capabilities().supports_currency(&Currency::JPY));
        assert!(provider.capabilities().supports_currency(&Currency::KWD));
        assert!(!PayPalOptions::empty()
            .build_accounts(Arc::new(SystemClock))
            .is_ok());
    }
}
