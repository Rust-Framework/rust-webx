//! Stripe provider, transport and registration.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use bytes::Bytes;
use rust_dix::IServiceResolverExt;
use serde_json::{json, Value};
use webx_client::{HttpClient, IHttpClient, OutboundRequest};
use webx_host::health::HealthCheckSpec;
use webx_host::health::HealthStatus;
use webx_host::server::HostBuilder;
use webx_pay::{
    AccountId, AccountSet, CaptureMode, ClientAction, CreatePayment, CurrencyScope,
    IPaymentProvider, IPaymentProviderBuilder, IPaymentProviderFactory, IPaymentWebhookVerifier,
    Money, PaymentAccount, PaymentError, PaymentFlow, PaymentFlowKind, PaymentIntent,
    PaymentMethod, PaymentOption, PaymentProviderConfig, PaymentScope, PaymentStatus,
    PaymentsServiceCollectionExt, ProviderCapabilities, ProviderId, Refund, RefundCommand,
    WebhookCapability,
};

use crate::form::{encode_form_pairs, encode_query_value};
use crate::mapping::{
    int_field, map_checkout_status, map_refund_status, map_status, money_from, order_id_from,
    string_field,
};
use crate::options::StripeOptions;
use crate::webhook::StripeWebhookVerifier;

const PROVIDER: ProviderId = ProviderId::Stripe;
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Stripe credentials for one merchant account.
#[derive(Clone)]
pub(crate) struct StripeAccount {
    pub(crate) id: AccountId,
    pub(crate) secret_key: String,
    pub(crate) publishable_key: Option<String>,
    pub(crate) webhook_secret: Option<String>,
    pub(crate) live: bool,
    pub(crate) api_version: Option<String>,
}

impl std::fmt::Debug for StripeAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StripeAccount")
            .field("id", &self.id)
            .field("secret_key", &"<redacted>")
            .field("publishable_key", &self.publishable_key)
            .field(
                "webhook_secret",
                &self.webhook_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("live", &self.live)
            .field("api_version", &self.api_version)
            .finish()
    }
}

#[derive(Debug, Clone, Copy)]
enum Verb {
    Get,
    Post,
}

/// Stripe adapter.
///
/// Publishes a frontend payment catalog through
/// [`IPaymentProvider::payment_options`]: one card option per configured
/// account, carrying only its publishable key as public client configuration.
pub struct StripeProvider {
    options: StripeOptions,
    accounts: Result<AccountSet<StripeAccount>, String>,
    verifier: StripeWebhookVerifier,
    injected_client: Option<Arc<dyn IHttpClient>>,
    client: OnceLock<Result<Arc<dyn IHttpClient>, String>>,
    validated: OnceLock<Result<(), String>>,
}

impl StripeProvider {
    /// Build a provider from options (configuration is validated at startup).
    pub fn new(options: StripeOptions) -> Self {
        let accounts = options.build_accounts();
        let verifier = StripeWebhookVerifier::new(accounts.clone());
        Self {
            options,
            accounts,
            verifier,
            injected_client: None,
            client: OnceLock::new(),
            validated: OnceLock::new(),
        }
    }

    /// Build a provider with an explicit HTTP client (tests, shared pool).
    pub fn with_client(options: StripeOptions, client: Arc<dyn IHttpClient>) -> Self {
        let mut provider = Self::new(options);
        provider.injected_client = Some(client);
        provider
    }

    fn account_set(&self) -> Result<&AccountSet<StripeAccount>, PaymentError> {
        self.accounts
            .as_ref()
            .map_err(|error| PaymentError::config(PROVIDER, error.clone()))
    }

    fn client(&self) -> Result<Arc<dyn IHttpClient>, PaymentError> {
        if let Some(client) = &self.injected_client {
            return Ok(Arc::clone(client));
        }
        let built = self.client.get_or_init(|| {
            HttpClient::new("stripe", self.options.http_options())
                .map(|client| Arc::new(client) as Arc<dyn IHttpClient>)
                .map_err(|error| error.to_string())
        });
        built
            .as_ref()
            .map(Arc::clone)
            .map_err(|error| PaymentError::config(PROVIDER, error.clone()))
    }

    /// Synchronous health probe: configuration parses and the HTTP client builds.
    pub fn health(&self) -> HealthStatus {
        match self.validate() {
            Ok(()) => HealthStatus::pass(),
            Err(error) => HealthStatus::fail(error.to_string()),
        }
    }

    fn account_for(
        &self,
        requested: Option<&AccountId>,
    ) -> Result<&Arc<StripeAccount>, PaymentError> {
        self.account_set()?.resolve(requested)
    }

    async fn send(
        &self,
        account: &StripeAccount,
        verb: Verb,
        path: String,
        form: &[(String, String)],
        idempotency_key: Option<&str>,
    ) -> Result<Value, PaymentError> {
        let client = self.client()?;
        let mut request = match verb {
            Verb::Get => OutboundRequest::get(path),
            Verb::Post => OutboundRequest::post(path),
        };
        request = request
            .header("authorization", format!("Bearer {}", account.secret_key))
            .header("accept", "application/json");
        if let Some(version) = &account.api_version {
            request = request.header("stripe-version", version.clone());
        }
        if let Some(key) = idempotency_key {
            request = request.header("idempotency-key", key.to_string());
        }
        if matches!(verb, Verb::Post) {
            request = request
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Bytes::from(encode_form_pairs(form)));
        }
        let response = client.execute(request).await?;
        let status = response.status;
        let request_id = response
            .headers
            .get("request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = response.body.collect(MAX_BODY_BYTES).await?;
        let value: Value = if body.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&body).map_err(|error| {
                PaymentError::Serialization(format!("invalid stripe response: {error}"))
            })?
        };
        if (200..300).contains(&status) {
            Ok(value)
        } else {
            Err(map_error(status, &value, request_id))
        }
    }

    async fn create_intent(
        &self,
        account: &StripeAccount,
        request: &CreatePayment,
        return_url: Option<&str>,
    ) -> Result<PaymentIntent, PaymentError> {
        let method = stripe_method(&request.method)?;
        if matches!(
            request.method,
            PaymentMethod::AlipayPage
                | PaymentMethod::AlipayWap
                | PaymentMethod::AlipayApp
                | PaymentMethod::AlipayQr
        ) && return_url.is_none()
        {
            return Err(PaymentError::validation(
                "stripe alipay requires a return_url in the client SDK flow",
            ));
        }
        let mut form = vec![
            ("amount".to_string(), request.amount.minor.to_string()),
            (
                "currency".to_string(),
                request.amount.currency.code().to_ascii_lowercase(),
            ),
            ("payment_method_types[]".to_string(), method.to_string()),
            ("confirm".to_string(), "false".to_string()),
            ("metadata[order_id]".to_string(), request.order_id.clone()),
            ("description".to_string(), request.description.clone()),
        ];
        if request.capture == CaptureMode::Manual {
            form.push(("capture_method".to_string(), "manual".to_string()));
        }
        if let Some(return_url) = return_url {
            form.push(("return_url".to_string(), return_url.to_string()));
        }
        append_metadata(&mut form, &request.metadata);
        let idempotency = request
            .idempotency_key
            .clone()
            .unwrap_or_else(|| request.order_id.clone());
        let value = self
            .send(
                account,
                Verb::Post,
                "/v1/payment_intents".to_string(),
                &form,
                Some(&idempotency),
            )
            .await?;
        self.parse_intent(&value, account)
    }

    async fn create_session(
        &self,
        account: &StripeAccount,
        request: &CreatePayment,
        success_url: &str,
        cancel_url: Option<&str>,
    ) -> Result<PaymentIntent, PaymentError> {
        let currency = request.amount.currency.code().to_ascii_lowercase();
        let title = if request.description.is_empty() {
            format!("Order {}", request.order_id)
        } else {
            request.description.clone()
        };
        let mut form = vec![
            ("mode".to_string(), "payment".to_string()),
            ("success_url".to_string(), success_url.to_string()),
            (
                "cancel_url".to_string(),
                cancel_url.unwrap_or(success_url).to_string(),
            ),
            ("client_reference_id".to_string(), request.order_id.clone()),
            ("line_items[0][quantity]".to_string(), "1".to_string()),
            ("line_items[0][price_data][currency]".to_string(), currency),
            (
                "line_items[0][price_data][unit_amount]".to_string(),
                request.amount.minor.to_string(),
            ),
            (
                "line_items[0][price_data][product_data][name]".to_string(),
                title,
            ),
            ("metadata[order_id]".to_string(), request.order_id.clone()),
            (
                "payment_intent_data[metadata][order_id]".to_string(),
                request.order_id.clone(),
            ),
        ];
        if request.capture == CaptureMode::Manual {
            form.push((
                "payment_intent_data[capture_method]".to_string(),
                "manual".to_string(),
            ));
        }
        append_metadata(&mut form, &request.metadata);
        let idempotency = request
            .idempotency_key
            .clone()
            .unwrap_or_else(|| request.order_id.clone());
        let value = self
            .send(
                account,
                Verb::Post,
                "/v1/checkout/sessions".to_string(),
                &form,
                Some(&idempotency),
            )
            .await?;
        self.parse_session(&value, account, None)
    }

    fn parse_intent(
        &self,
        value: &Value,
        account: &StripeAccount,
    ) -> Result<PaymentIntent, PaymentError> {
        let id = string_field(value, "id").ok_or_else(|| {
            PaymentError::Serialization("stripe payment intent has no id".to_string())
        })?;
        let status_raw = string_field(value, "status").unwrap_or_default();
        let currency = string_field(value, "currency");
        let amount = money_from(int_field(value, "amount"), currency).ok_or_else(|| {
            PaymentError::Serialization("stripe payment intent has no amount/currency".to_string())
        })?;
        let order_id = order_id_from(value).unwrap_or_default();
        let mut intent = PaymentIntent::new(
            PROVIDER,
            account.id.clone(),
            order_id,
            amount,
            map_status(status_raw),
            status_raw,
        );
        intent.provider_payment_id = Some(id.to_string());
        intent.amount_captured = money_from(int_field(value, "amount_received"), currency);
        intent.created_at = int_field(value, "created").unwrap_or(0);
        intent.metadata = parse_metadata(value);
        intent.client_action = intent_client_action(value);
        Ok(intent)
    }

    fn parse_session(
        &self,
        value: &Value,
        account: &StripeAccount,
        request_id: Option<String>,
    ) -> Result<PaymentIntent, PaymentError> {
        let id = string_field(value, "id").ok_or_else(|| {
            PaymentError::Serialization("stripe checkout session has no id".to_string())
        })?;
        let status_raw = string_field(value, "status").unwrap_or_default();
        let payment_status = string_field(value, "payment_status").unwrap_or_default();
        let currency = string_field(value, "currency");
        let amount = money_from(int_field(value, "amount_total"), currency).ok_or_else(|| {
            PaymentError::Serialization(
                "stripe checkout session has no amount_total/currency".to_string(),
            )
        })?;
        let order_id = order_id_from(value).unwrap_or_default();
        let mut intent = PaymentIntent::new(
            PROVIDER,
            account.id.clone(),
            order_id,
            amount,
            map_checkout_status(status_raw, payment_status),
            status_raw,
        );
        intent.provider_payment_id = Some(
            string_field(value, "payment_intent")
                .unwrap_or(id)
                .to_string(),
        );
        intent.created_at = int_field(value, "created").unwrap_or(0);
        intent.expires_at = int_field(value, "expires_at");
        intent.metadata = parse_metadata(value);
        intent.request_id = request_id;
        if let Some(url) = string_field(value, "url") {
            intent.client_action = Some(ClientAction::Redirect {
                url: url.to_string(),
            });
        }
        Ok(intent)
    }

    async fn intent_id_for_order(
        &self,
        account: &StripeAccount,
        order_id: &str,
    ) -> Result<String, PaymentError> {
        if order_id.starts_with("pi_") {
            return Ok(order_id.to_string());
        }
        let intent = self.get_payment_impl(account, order_id, true).await?;
        let id = intent
            .provider_payment_id
            .ok_or_else(|| PaymentError::NotFound {
                provider: PROVIDER,
                message: "payment has no provider id yet".to_string(),
            })?;
        if id.starts_with("pi_") {
            Ok(id)
        } else if id.starts_with("cs_") {
            let value = self
                .send(
                    account,
                    Verb::Get,
                    format!("/v1/checkout/sessions/{id}"),
                    &[],
                    None,
                )
                .await?;
            string_field(&value, "payment_intent")
                .map(str::to_string)
                .ok_or_else(|| PaymentError::NotFound {
                    provider: PROVIDER,
                    message: "checkout session has no completed payment".to_string(),
                })
        } else {
            Err(PaymentError::NotFound {
                provider: PROVIDER,
                message: "payment has no payment intent".to_string(),
            })
        }
    }

    async fn get_payment_impl(
        &self,
        account: &StripeAccount,
        order_id: &str,
        allow_empty: bool,
    ) -> Result<PaymentIntent, PaymentError> {
        if let Some(rest) = order_id.strip_prefix("pi_") {
            let value = self
                .send(
                    account,
                    Verb::Get,
                    format!("/v1/payment_intents/pi_{}", safe_id(rest)?),
                    &[],
                    None,
                )
                .await?;
            return self.parse_intent(&value, account);
        }
        if let Some(rest) = order_id.strip_prefix("cs_") {
            let value = self
                .send(
                    account,
                    Verb::Get,
                    format!("/v1/checkout/sessions/cs_{}", safe_id(rest)?),
                    &[],
                    None,
                )
                .await?;
            return self.parse_session(&value, account, None);
        }
        let query = format!("metadata['order_id']:'{}'", escape_search(order_id));
        let value = self
            .send(
                account,
                Verb::Get,
                format!(
                    "/v1/payment_intents/search?query={}",
                    encode_query_value(&query)
                ),
                &[],
                None,
            )
            .await?;
        let item = value
            .get("data")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .cloned();
        match item {
            Some(item) => self.parse_intent(&item, account),
            None if allow_empty => Err(PaymentError::NotFound {
                provider: PROVIDER,
                message: format!("no stripe payment found for order `{order_id}`"),
            }),
            None => Err(PaymentError::NotFound {
                provider: PROVIDER,
                message: format!("no stripe payment found for order `{order_id}`"),
            }),
        }
    }

    fn parse_refund(
        &self,
        value: &Value,
        account: &StripeAccount,
        order_id: &str,
    ) -> Result<Refund, PaymentError> {
        let id = string_field(value, "id")
            .ok_or_else(|| PaymentError::Serialization("stripe refund has no id".to_string()))?;
        let status_raw = string_field(value, "status").unwrap_or_default();
        let currency = string_field(value, "currency");
        let amount = money_from(int_field(value, "amount"), currency).ok_or_else(|| {
            PaymentError::Serialization("stripe refund has no amount/currency".to_string())
        })?;
        Ok(Refund {
            provider: PROVIDER,
            account: account.id.clone(),
            order_id: order_id.to_string(),
            refund_id: parse_metadata(value)
                .get("refund_id")
                .cloned()
                .unwrap_or_else(|| id.to_string()),
            provider_refund_id: Some(id.to_string()),
            status: map_refund_status(status_raw),
            provider_status: status_raw.to_string(),
            amount,
            created_at: int_field(value, "created").unwrap_or(0),
            succeeded_at: None,
            request_id: None,
        })
    }
}

#[async_trait]
impl IPaymentProvider for StripeProvider {
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

    /// One card option per configured account.
    ///
    /// The public client configuration carries only the account's publishable
    /// key, when configured; secret credentials never enter the catalog.
    fn payment_options(&self) -> Vec<PaymentOption> {
        let Ok(accounts) = &self.accounts else {
            return Vec::new();
        };
        accounts
            .iter()
            .map(|(id, account)| {
                let mut option = PaymentOption::new(
                    PROVIDER,
                    id.clone(),
                    PaymentMethod::Card,
                    [PaymentFlowKind::ClientSdk, PaymentFlowKind::Redirect],
                );
                if let Some(publishable_key) = &account.publishable_key {
                    option =
                        option.with_client_config(json!({ "publishable_key": publishable_key }));
                }
                if let Some(label) = accounts
                    .accounts()
                    .iter()
                    .find(|info| &info.id == id)
                    .and_then(|info| info.label.clone())
                {
                    option = option.with_label(label);
                }
                option
            })
            .collect()
    }

    fn validate(&self) -> Result<(), PaymentError> {
        let result = self.validated.get_or_init(|| {
            self.account_set().map_err(|error| error.to_string())?;
            self.client().map_err(|error| error.to_string())?;
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
        match &request.flow {
            PaymentFlow::ClientSdk { return_url } => {
                self.create_intent(account, &request, return_url.as_deref())
                    .await
            }
            PaymentFlow::Redirect {
                success_url,
                cancel_url,
            } => {
                self.create_session(account, &request, success_url, cancel_url.as_deref())
                    .await
            }
            PaymentFlow::QrCode => Err(PaymentError::unsupported(PROVIDER, "flow.qr_code")),
            PaymentFlow::App { .. } => Err(PaymentError::unsupported(PROVIDER, "flow.app")),
        }
    }

    async fn get_payment(
        &self,
        scope: &PaymentScope,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        let account = self.account_for(scope.account_ref())?;
        self.get_payment_impl(account, order_id, false).await
    }

    async fn capture_payment(
        &self,
        scope: &PaymentScope,
        order_id: &str,
        amount: Option<Money>,
    ) -> Result<PaymentIntent, PaymentError> {
        let account = self.account_for(scope.account_ref())?;
        let id = self.intent_id_for_order(account, order_id).await?;
        let mut form = Vec::new();
        if let Some(amount) = amount {
            form.push(("amount_to_capture".to_string(), amount.minor.to_string()));
        }
        let value = self
            .send(
                account,
                Verb::Post,
                format!("/v1/payment_intents/{id}/capture"),
                &form,
                Some(order_id),
            )
            .await?;
        self.parse_intent(&value, account)
    }

    async fn cancel_payment(
        &self,
        scope: &PaymentScope,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        let account = self.account_for(scope.account_ref())?;
        if let Some(rest) = order_id.strip_prefix("cs_") {
            let value = self
                .send(
                    account,
                    Verb::Post,
                    format!("/v1/checkout/sessions/cs_{}/expire", safe_id(rest)?),
                    &[],
                    Some(order_id),
                )
                .await?;
            return self.parse_session(&value, account, None);
        }
        let intent = self.get_payment_impl(account, order_id, false).await?;
        let provider_id = intent
            .provider_payment_id
            .ok_or_else(|| PaymentError::NotFound {
                provider: PROVIDER,
                message: "payment has no provider id".to_string(),
            })?;
        if provider_id.starts_with("cs_") {
            let value = self
                .send(
                    account,
                    Verb::Post,
                    format!("/v1/checkout/sessions/{provider_id}/expire"),
                    &[],
                    Some(order_id),
                )
                .await?;
            self.parse_session(&value, account, None)
        } else {
            let value = self
                .send(
                    account,
                    Verb::Post,
                    format!("/v1/payment_intents/{provider_id}/cancel"),
                    &[],
                    Some(order_id),
                )
                .await?;
            self.parse_intent(&value, account)
        }
    }

    async fn refund(
        &self,
        scope: &PaymentScope,
        request: RefundCommand,
    ) -> Result<Refund, PaymentError> {
        request.validate()?;
        let account = self.account_for(scope.account_ref())?;
        let intent_id = self.intent_id_for_order(account, &request.order_id).await?;
        let mut form = vec![
            ("payment_intent".to_string(), intent_id),
            ("metadata[refund_id]".to_string(), request.refund_id.clone()),
            ("metadata[order_id]".to_string(), request.order_id.clone()),
        ];
        if let Some(amount) = &request.amount {
            form.push(("amount".to_string(), amount.minor.to_string()));
        }
        if matches!(
            request.reason.as_str(),
            "duplicate" | "fraudulent" | "requested_by_customer"
        ) {
            form.push(("reason".to_string(), request.reason.clone()));
        }
        let idempotency = request
            .idempotency_key
            .clone()
            .unwrap_or_else(|| request.refund_id.clone());
        let value = self
            .send(
                account,
                Verb::Post,
                "/v1/refunds".to_string(),
                &form,
                Some(&idempotency),
            )
            .await?;
        self.parse_refund(&value, account, &request.order_id)
    }

    async fn get_refund(
        &self,
        scope: &PaymentScope,
        refund_id: &str,
    ) -> Result<Refund, PaymentError> {
        let account = self.account_for(scope.account_ref())?;
        let Some(rest) = refund_id.strip_prefix("re_") else {
            return Err(PaymentError::validation(
                "stripe refund lookup requires the provider refund id (`re_...`)",
            ));
        };
        let value = self
            .send(
                account,
                Verb::Get,
                format!("/v1/refunds/re_{}", safe_id(rest)?),
                &[],
                None,
            )
            .await?;
        let order_id = parse_metadata(&value)
            .get("order_id")
            .cloned()
            .unwrap_or_default();
        self.parse_refund(&value, account, &order_id)
    }

    fn webhook_verifier(&self) -> &dyn IPaymentWebhookVerifier {
        &self.verifier
    }
}

fn append_metadata(
    form: &mut Vec<(String, String)>,
    metadata: &std::collections::BTreeMap<String, String>,
) {
    for (key, value) in metadata {
        form.push((format!("metadata[{key}]"), value.clone()));
    }
}

fn parse_metadata(value: &Value) -> std::collections::BTreeMap<String, String> {
    value
        .get("metadata")
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn intent_client_action(value: &Value) -> Option<ClientAction> {
    let next = value.get("next_action");
    if let Some(url) = next
        .and_then(|next| next.get("redirect_to_url"))
        .and_then(|redirect| redirect.get("url"))
        .and_then(Value::as_str)
    {
        return Some(ClientAction::Redirect {
            url: url.to_string(),
        });
    }
    let id = string_field(value, "id")?;
    let client_secret = string_field(value, "client_secret")?;
    Some(ClientAction::ClientSdk {
        provider: PROVIDER,
        payload: json!({
            "client_secret": client_secret,
            "payment_intent_id": id,
        }),
    })
}

fn stripe_method(method: &PaymentMethod) -> Result<&'static str, PaymentError> {
    match method {
        PaymentMethod::Card | PaymentMethod::ApplePay | PaymentMethod::GooglePay => Ok("card"),
        PaymentMethod::PayPal => Ok("paypal"),
        PaymentMethod::AlipayPage
        | PaymentMethod::AlipayWap
        | PaymentMethod::AlipayApp
        | PaymentMethod::AlipayQr => Ok("alipay"),
        PaymentMethod::WechatNative
        | PaymentMethod::WechatJsapi
        | PaymentMethod::WechatApp
        | PaymentMethod::WechatH5 => Err(PaymentError::unsupported(PROVIDER, "wechat_pay")),
        PaymentMethod::Other(name) => {
            let _ = name;
            Err(PaymentError::unsupported(PROVIDER, "payment_method"))
        }
    }
}

fn safe_id(id: &str) -> Result<&str, PaymentError> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(PaymentError::validation("invalid stripe object id"));
    }
    Ok(id)
}

fn escape_search(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

fn map_error(status: u16, value: &Value, request_id: Option<String>) -> PaymentError {
    let error = value.get("error");
    let kind = error
        .and_then(|error| error.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let code = error
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("stripe request failed")
        .to_string();
    if status == 404 {
        return PaymentError::not_found(PROVIDER, message);
    }
    if kind == "idempotency_error" || code.as_deref() == Some("idempotency_error") {
        return PaymentError::conflict(PROVIDER, message);
    }
    let retryable = status == 429 || status >= 500 || kind == "api_error";
    PaymentError::provider(PROVIDER, code, message, Some(status), request_id, retryable)
}

/// Register Stripe on a [`HostBuilder`].
pub trait StripeHostBuilderExt: Sized {
    /// Register the Stripe adapter with runtime configuration.
    ///
    /// Binds the `"Stripe"` configuration section through
    /// `add_options`, so appsettings/file reloads are picked up by the
    /// runtime cache (default 60s) or immediately after
    /// `Payments::invalidate`.
    fn add_stripe(self) -> Self;

    /// Register the adapter with a prebuilt static configuration.
    fn add_stripe_with(self, options: StripeOptions) -> Self;
}

impl StripeHostBuilderExt for HostBuilder {
    fn add_stripe(self) -> Self {
        let builder = self.add_options::<StripeOptions>("Stripe");
        builder.register(|services| {
            services
                .add_payment_provider_factory_lazy(|resolver| {
                    let monitor = resolver
                        .resolve_arc::<dyn webx_core::options_monitor::IOptionsMonitor<StripeOptions>>()
                        .expect("add_stripe() registers the Stripe options monitor");
                    Arc::new(StripeProviderFactory::new(monitor))
                })
                .add_payment_provider_builder(Arc::new(StripeProviderBuilder))
        })
    }

    fn add_stripe_with(self, options: StripeOptions) -> Self {
        let provider = Arc::new(StripeProvider::new(options));
        let health = Arc::clone(&provider);
        self.register(move |services| {
            services
                .add_payment_provider(provider)
                .add_payment_provider_builder(Arc::new(StripeProviderBuilder))
        })
        .add_health_check(HealthCheckSpec::sync("payments.stripe", move || {
            health.health()
        }))
    }
}

/// Runtime provider factory for Stripe (reads the options monitor).
pub struct StripeProviderFactory {
    monitor: Arc<dyn webx_core::options_monitor::IOptionsMonitor<StripeOptions>>,
}

impl StripeProviderFactory {
    /// Build the factory over an options monitor.
    pub fn new(
        monitor: Arc<dyn webx_core::options_monitor::IOptionsMonitor<StripeOptions>>,
    ) -> Self {
        Self { monitor }
    }
}

#[async_trait]
impl IPaymentProviderFactory for StripeProviderFactory {
    fn provider(&self) -> ProviderId {
        PROVIDER
    }

    async fn resolve(
        &self,
        _scope: &PaymentScope,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        let options = self.monitor.current();
        let provider = StripeProvider::new((*options).clone());
        provider.validate()?;
        Ok(Arc::new(provider))
    }
}

/// Builder used by custom `IPaymentServiceFactory` implementations.
#[derive(Debug, Default, Clone, Copy)]
pub struct StripeProviderBuilder;

impl IPaymentProviderBuilder for StripeProviderBuilder {
    fn provider(&self) -> ProviderId {
        PROVIDER
    }

    fn build(
        &self,
        config: &PaymentProviderConfig,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        let options: StripeOptions =
            serde_json::from_value(config.settings.clone()).map_err(|error| {
                PaymentError::config(
                    PROVIDER,
                    format!("invalid Stripe provider settings: {error}"),
                )
            })?;
        let provider = StripeProvider::new(options);
        provider.validate()?;
        Ok(Arc::new(provider))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_ids_reject_path_characters() {
        assert!(safe_id("abc_123").is_ok());
        assert!(safe_id("../x").is_err());
        assert!(safe_id("a b").is_err());
    }

    #[test]
    fn metadata_is_flattened_into_form_pairs() {
        let mut form = Vec::new();
        let metadata = [("user_id".to_string(), "42".to_string())]
            .into_iter()
            .collect();
        append_metadata(&mut form, &metadata);
        assert_eq!(
            form,
            vec![("metadata[user_id]".to_string(), "42".to_string())]
        );
    }

    fn options_with_publishable_key() -> StripeOptions {
        let mut options = StripeOptions::new("sk_test_super_secret", "whsec_super_secret");
        options.accounts[0] = options.accounts[0]
            .clone()
            .with_publishable_key("pk_test_public");
        options
    }

    #[test]
    fn payment_options_describe_the_fixture_account() {
        let catalog = StripeProvider::new(options_with_publishable_key()).payment_options();
        assert_eq!(catalog.len(), 1);
        let option = &catalog[0];
        assert_eq!(option.provider, ProviderId::Stripe);
        assert_eq!(option.account, AccountId::default_account());
        assert_eq!(option.method, PaymentMethod::Card);
        assert_eq!(
            option.flows,
            vec![PaymentFlowKind::ClientSdk, PaymentFlowKind::Redirect]
        );
        assert_eq!(
            option.client_config,
            Some(json!({ "publishable_key": "pk_test_public" }))
        );
        assert!(option.label.is_none());
    }

    #[test]
    fn payment_options_cover_every_configured_account() {
        let options = StripeOptions::empty()
            .with_account("a", "sk_test_a", None)
            .with_account("b", "sk_test_b", None);
        let catalog = StripeProvider::new(options).payment_options();
        assert_eq!(catalog.len(), 2);
        let mut ids: Vec<&str> = catalog
            .iter()
            .map(|option| option.account.as_str())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn client_config_is_only_present_with_a_publishable_key() {
        let catalog =
            StripeProvider::new(StripeOptions::new("sk_test_secret", "")).payment_options();
        assert_eq!(catalog.len(), 1);
        assert!(catalog[0].client_config.is_none());

        let catalog = StripeProvider::new(options_with_publishable_key()).payment_options();
        assert!(catalog[0].client_config.is_some());
    }

    #[test]
    fn serialized_catalog_never_exposes_secret_credentials() {
        let catalog = StripeProvider::new(options_with_publishable_key()).payment_options();
        let text = serde_json::to_string(&catalog).unwrap();
        assert!(text.contains("pk_test_public"));
        assert!(!text.contains("sk_test_super_secret"));
        assert!(!text.contains("whsec_super_secret"));
    }

    #[tokio::test]
    async fn stripe_satisfies_the_payment_provider_contract() {
        let provider = StripeProvider::new(StripeOptions::new("fixture-key", "fixture-webhook"));
        webx_pay::testing::payment_provider_contract(&provider).await;
    }
}
