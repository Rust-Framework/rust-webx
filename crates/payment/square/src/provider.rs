//! Square provider: Payments API operations and host registration.
//!
//! Implemented operation surface (Square REST v2):
//!
//! | Operation | Request |
//! |---|---|
//! | create (client SDK) | `POST /v2/payments` |
//! | create (hosted redirect) | `POST /v2/online-checkout/payment-links` |
//! | query | `GET /v2/payments/{payment_id}` |
//! | capture (delayed capture) | `POST /v2/payments/{payment_id}/complete` |
//! | cancel | `POST /v2/payments/{payment_id}/cancel` |
//! | refund | `POST /v2/refunds` |
//! | query refund | `GET /v2/refunds/{refund_id}` |
//!
//! # Client-side token requirement
//!
//! Square never accepts raw card data on the server. The browser must tokenize
//! the card with the Web Payments SDK (or Apple Pay / Google Pay) and the
//! resulting token is sent as `source_id`. The adapter reads that token from
//! `CreatePayment::metadata["square_source_id"]` and returns a validation error
//! when it is missing.
//!
//! # Identifiers
//!
//! Square has no payment lookup by merchant order reference (the
//! `ListPayments` workaround needs a time window and a location). Query,
//! capture, cancel and refund therefore expect the Square **payment id**
//! (`PaymentIntent::provider_payment_id`) in the `order_id` position, and
//! return a validation error pointing that out when the value cannot be a
//! Square object id.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use rust_dix::IServiceResolverExt;
use serde_json::{json, Value};
use webx_client::{HttpClient, IHttpClient};
use webx_host::health::HealthStatus;
use webx_host::server::HostBuilder;
use webx_pay::{
    AccountId, AccountSet, CaptureMode, ClientAction, CreatePayment, CurrencyScope,
    IPaymentProvider, IPaymentProviderBuilder, IPaymentProviderFactory, IPaymentWebhookVerifier,
    Money, PaymentAccount, PaymentError, PaymentFlow, PaymentFlowKind, PaymentIntent,
    PaymentMethod, PaymentOption, PaymentProviderConfig, PaymentScope, PaymentStatus,
    PaymentsServiceCollectionExt, ProviderCapabilities, ProviderId, Refund, RefundCommand,
    RefundStatus, WebhookCapability,
};

use crate::mapping::{
    map_payment_status, map_refund_status, money_from, money_json, parse_timestamp, string_field,
    unix_now,
};
use crate::options::SquareOptions;
use crate::transport::{build_request, derive_idempotency_key, execute, SquareRequest, Verb};
use crate::webhook::SquareWebhookVerifier;

/// Provider id owned by this adapter.
const PROVIDER: ProviderId = ProviderId::Square;

/// `CreatePayment::metadata` key carrying the Web Payments SDK token (`source_id`).
pub const SOURCE_ID_METADATA: &str = "square_source_id";

/// Square limits a payment `reference_id` to 40 characters.
const PAYMENT_REFERENCE_ID_MAX: usize = 40;
/// Square limits a payment `note` to 500 characters.
const NOTE_MAX: usize = 500;
/// Square limits a refund `reason` to 192 characters.
const REFUND_REASON_MAX: usize = 192;
/// Square limits a payment-link description to 4096 characters.
const LINK_DESCRIPTION_MAX: usize = 4096;
/// Conservative cap for a payment-link line-item name.
const LINK_ITEM_NAME_MAX: usize = 500;
/// Square object ids are opaque; values shorter than this are merchant ids.
const MIN_PROVIDER_ID_LEN: usize = 16;

/// Square credentials for one merchant account.
#[derive(Clone)]
pub(crate) struct SquareAccount {
    pub(crate) id: AccountId,
    pub(crate) access_token: String,
    pub(crate) signature_key: Option<String>,
    pub(crate) location_id: Option<String>,
    pub(crate) application_id: Option<String>,
    pub(crate) live: bool,
    pub(crate) api_version: String,
    pub(crate) base_url: String,
}

impl std::fmt::Debug for SquareAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SquareAccount")
            .field("id", &self.id)
            .field("access_token", &"<redacted>")
            .field(
                "signature_key",
                &self.signature_key.as_ref().map(|_| "<redacted>"),
            )
            .field("location_id", &self.location_id)
            .field("application_id", &self.application_id)
            .field("live", &self.live)
            .field("api_version", &self.api_version)
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// Square adapter.
pub struct SquareProvider {
    options: SquareOptions,
    accounts: Result<AccountSet<SquareAccount>, String>,
    verifier: SquareWebhookVerifier,
    injected_client: Option<Arc<dyn IHttpClient>>,
    client: OnceLock<Result<Arc<dyn IHttpClient>, String>>,
    validated: OnceLock<Result<(), String>>,
}

impl SquareProvider {
    /// Build a provider from options (configuration is validated at startup).
    pub fn new(options: SquareOptions) -> Self {
        let accounts = options.build_accounts();
        let verifier = SquareWebhookVerifier::new(accounts.clone(), options.webhook_url());
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
    pub fn with_client(options: SquareOptions, client: Arc<dyn IHttpClient>) -> Self {
        let mut provider = Self::new(options);
        provider.injected_client = Some(client);
        provider
    }

    fn account_set(&self) -> Result<&AccountSet<SquareAccount>, PaymentError> {
        self.accounts
            .as_ref()
            .map_err(|error| PaymentError::config(PROVIDER, error.clone()))
    }

    fn client(&self) -> Result<Arc<dyn IHttpClient>, PaymentError> {
        if let Some(client) = &self.injected_client {
            return Ok(Arc::clone(client));
        }
        let built = self.client.get_or_init(|| {
            HttpClient::new("square", self.options.http_options())
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
    ) -> Result<&Arc<SquareAccount>, PaymentError> {
        self.account_set()?.resolve(requested)
    }

    async fn send(
        &self,
        account: &SquareAccount,
        request: SquareRequest,
    ) -> Result<Value, PaymentError> {
        let client = self.client()?;
        execute(&client, account, request).await
    }

    async fn create_with_source(
        &self,
        account: &SquareAccount,
        request: &CreatePayment,
    ) -> Result<PaymentIntent, PaymentError> {
        let body = build_payment_body(account, request)?;
        let value = self
            .send(
                account,
                build_request(account, Verb::Post, "/v2/payments", Some(&body))?,
            )
            .await?;
        let mut intent = parse_payment(account, &value)?;
        let provider_payment_id = intent.provider_payment_id.clone();
        let provider_status = intent.provider_status.clone();
        intent.client_action = Some(ClientAction::ClientSdk {
            provider: PROVIDER,
            payload: json!({
                "payment_id": provider_payment_id,
                "status": provider_status,
                "location_id": account.location_id,
            }),
        });
        Ok(intent)
    }

    async fn create_link(
        &self,
        account: &SquareAccount,
        request: &CreatePayment,
        success_url: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        let body = build_payment_link_body(account, request, success_url)?;
        let value = self
            .send(
                account,
                build_request(
                    account,
                    Verb::Post,
                    "/v2/online-checkout/payment-links",
                    Some(&body),
                )?,
            )
            .await?;
        parse_payment_link(account, &value, request)
    }

    /// Total amount still refundable on `payment_id`.
    ///
    /// `RefundPayment` requires an explicit `amount_money`; when the caller
    /// asked for a full refund this reads the payment and subtracts
    /// `refunded_money`.
    async fn outstanding_amount(
        &self,
        account: &SquareAccount,
        payment_id: &str,
    ) -> Result<Money, PaymentError> {
        let value = self
            .send(
                account,
                build_request(
                    account,
                    Verb::Get,
                    format!("/v2/payments/{payment_id}"),
                    None,
                )?,
            )
            .await?;
        let payment = value
            .get("payment")
            .filter(|value| value.is_object())
            .ok_or_else(|| {
                PaymentError::Serialization(
                    "square payment response has no payment object".to_string(),
                )
            })?;
        let amount = money_from(payment.get("amount_money")).ok_or_else(|| {
            PaymentError::Serialization("square payment has no amount_money".to_string())
        })?;
        let refunded = money_from(payment.get("refunded_money"))
            .unwrap_or_else(|| Money::zero(amount.currency));
        let remaining = amount.checked_sub(refunded).map_err(|_| {
            PaymentError::Serialization("square payment currency mismatch".to_string())
        })?;
        if remaining.minor <= 0 {
            return Err(PaymentError::validation(format!(
                "Square payment `{payment_id}` has nothing left to refund"
            )));
        }
        Ok(remaining)
    }
}

#[async_trait]
impl IPaymentProvider for SquareProvider {
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

    fn payment_options(&self) -> Vec<PaymentOption> {
        let Ok(accounts) = &self.accounts else {
            return Vec::new();
        };
        accounts
            .iter()
            .map(|(id, account)| {
                // Only public Square identifiers reach the frontend SDK: the
                // access token and the webhook signature key never do.
                let mut client_config = json!({
                    "environment": if account.live { "production" } else { "sandbox" },
                });
                if let Some(application_id) = &account.application_id {
                    client_config["application_id"] = Value::String(application_id.clone());
                }
                if let Some(location_id) = &account.location_id {
                    client_config["location_id"] = Value::String(location_id.clone());
                }
                let label = accounts
                    .accounts()
                    .iter()
                    .find(|info| &info.id == id)
                    .and_then(|info| info.label.clone());
                let option = PaymentOption::new(
                    PROVIDER,
                    id.clone(),
                    PaymentMethod::Card,
                    [PaymentFlowKind::ClientSdk],
                )
                .with_client_config(client_config);
                match label {
                    Some(label) => option.with_label(label),
                    None => option,
                }
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
            PaymentFlow::ClientSdk { .. } => self.create_with_source(account, &request).await,
            PaymentFlow::Redirect { success_url, .. } => {
                self.create_link(account, &request, success_url).await
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
        let payment_id = payment_id_arg(order_id)?;
        let value = self
            .send(
                account,
                build_request(
                    account,
                    Verb::Get,
                    format!("/v2/payments/{payment_id}"),
                    None,
                )?,
            )
            .await?;
        parse_payment(account, &value)
    }

    async fn capture_payment(
        &self,
        scope: &PaymentScope,
        order_id: &str,
        amount: Option<Money>,
    ) -> Result<PaymentIntent, PaymentError> {
        let account = self.account_for(scope.account_ref())?;
        let payment_id = payment_id_arg(order_id)?;
        if amount.is_some() {
            return Err(PaymentError::validation(
                "Square delayed capture completes the full approved amount; partial capture is not supported (omit the amount)",
            ));
        }
        let value = self
            .send(
                account,
                build_request(
                    account,
                    Verb::Post,
                    format!("/v2/payments/{payment_id}/complete"),
                    Some(&json!({})),
                )?,
            )
            .await?;
        parse_payment(account, &value)
    }

    async fn cancel_payment(
        &self,
        scope: &PaymentScope,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        let account = self.account_for(scope.account_ref())?;
        let payment_id = payment_id_arg(order_id)?;
        let value = self
            .send(
                account,
                build_request(
                    account,
                    Verb::Post,
                    format!("/v2/payments/{payment_id}/cancel"),
                    Some(&json!({})),
                )?,
            )
            .await?;
        parse_payment(account, &value)
    }

    async fn refund(
        &self,
        scope: &PaymentScope,
        request: RefundCommand,
    ) -> Result<Refund, PaymentError> {
        request.validate()?;
        let account = self.account_for(scope.account_ref())?;
        let payment_id = payment_id_arg(&request.order_id)?;
        let amount = match request.amount {
            Some(amount) => amount,
            None => self.outstanding_amount(account, payment_id).await?,
        };
        let body = build_refund_body(&request, payment_id, &amount);
        let value = self
            .send(
                account,
                build_request(account, Verb::Post, "/v2/refunds", Some(&body))?,
            )
            .await?;
        parse_refund(account, &value, &request.order_id, &request.refund_id)
    }

    async fn get_refund(
        &self,
        scope: &PaymentScope,
        refund_id: &str,
    ) -> Result<Refund, PaymentError> {
        let account = self.account_for(scope.account_ref())?;
        let refund_id = refund_id_arg(refund_id)?;
        let value = self
            .send(
                account,
                build_request(account, Verb::Get, format!("/v2/refunds/{refund_id}"), None)?,
            )
            .await?;
        parse_refund(account, &value, "", refund_id)
    }

    fn webhook_verifier(&self) -> &dyn IPaymentWebhookVerifier {
        &self.verifier
    }
}

/// Reject malformed/overlong values before they land in a URL path.
fn object_id(value: &str) -> Option<&str> {
    let value = value.trim();
    let path_safe = !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    (path_safe && value.len() >= MIN_PROVIDER_ID_LEN).then_some(value)
}

fn payment_id_arg(order_id: &str) -> Result<&str, PaymentError> {
    object_id(order_id).ok_or_else(|| {
        PaymentError::validation(format!(
            "Square has no lookup by merchant order reference; pass the Square payment id (`PaymentIntent::provider_payment_id`) instead of `{order_id}`"
        ))
    })
}

fn refund_id_arg(refund_id: &str) -> Result<&str, PaymentError> {
    object_id(refund_id).ok_or_else(|| {
        PaymentError::validation(format!(
            "Square refund lookup requires the Square refund id (`Refund::provider_refund_id`) instead of `{refund_id}`"
        ))
    })
}

fn require_location(account: &SquareAccount) -> Result<&str, PaymentError> {
    account.location_id.as_deref().ok_or_else(|| {
        PaymentError::validation(format!(
            "square account `{}` has no location_id; set SquareAccountOptions::with_location_id",
            account.id
        ))
    })
}

fn ensure_supported_method(method: &PaymentMethod) -> Result<(), PaymentError> {
    match method {
        // Apple Pay / Google Pay are tokenized by the Web Payments SDK as well.
        PaymentMethod::Card | PaymentMethod::ApplePay | PaymentMethod::GooglePay => Ok(()),
        _ => Err(PaymentError::unsupported(PROVIDER, "payment_method")),
    }
}

fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

/// `POST /v2/payments` body for the Web Payments SDK flow.
fn build_payment_body(
    account: &SquareAccount,
    request: &CreatePayment,
) -> Result<Value, PaymentError> {
    ensure_supported_method(&request.method)?;
    let source_id = request
        .metadata
        .get(SOURCE_ID_METADATA)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            PaymentError::validation(format!(
                "Square payments require a client-side payment token: tokenize the card with the Web Payments SDK and pass the token in CreatePayment::metadata[\"{SOURCE_ID_METADATA}\"]"
            ))
        })?;
    let location_id = require_location(account)?;
    if request.order_id.chars().count() > PAYMENT_REFERENCE_ID_MAX {
        return Err(PaymentError::validation(format!(
            "Square payment reference_id is limited to {PAYMENT_REFERENCE_ID_MAX} characters; order_id `{}` is longer",
            request.order_id
        )));
    }
    let mut body = json!({
        "idempotency_key": derive_idempotency_key(
            request.idempotency_key.as_deref(),
            &request.order_id,
        ),
        "location_id": location_id,
        "source_id": source_id,
        "amount_money": money_json(&request.amount),
        "reference_id": request.order_id,
        "autocomplete": request.capture == CaptureMode::Automatic,
    });
    let note = truncate_chars(request.description.trim(), NOTE_MAX);
    if !note.is_empty() {
        body["note"] = Value::String(note);
    }
    Ok(body)
}

/// `POST /v2/online-checkout/payment-links` body for the hosted redirect flow.
fn build_payment_link_body(
    account: &SquareAccount,
    request: &CreatePayment,
    success_url: &str,
) -> Result<Value, PaymentError> {
    ensure_supported_method(&request.method)?;
    if request.capture == CaptureMode::Manual {
        return Err(PaymentError::unsupported(
            PROVIDER,
            "manual capture with payment links",
        ));
    }
    let success_url = success_url.trim();
    if success_url.is_empty() {
        return Err(PaymentError::validation(
            "Square payment links require a non-empty success_url",
        ));
    }
    let location_id = require_location(account)?;
    let title = match truncate_chars(request.description.trim(), LINK_ITEM_NAME_MAX) {
        title if title.is_empty() => format!("Order {}", request.order_id),
        title => title,
    };
    let mut body = json!({
        "idempotency_key": derive_idempotency_key(
            request.idempotency_key.as_deref(),
            &request.order_id,
        ),
        "order": {
            "location_id": location_id,
            "reference_id": request.order_id,
            "line_items": [{
                "name": title,
                "quantity": "1",
                "base_price_money": money_json(&request.amount),
            }],
        },
        "checkout_options": { "redirect_url": success_url },
    });
    let description = truncate_chars(request.description.trim(), LINK_DESCRIPTION_MAX);
    if !description.is_empty() {
        body["description"] = Value::String(description);
    }
    Ok(body)
}

/// `POST /v2/refunds` body.
fn build_refund_body(request: &RefundCommand, payment_id: &str, amount: &Money) -> Value {
    let mut body = json!({
        "idempotency_key": derive_idempotency_key(
            request.idempotency_key.as_deref(),
            &request.refund_id,
        ),
        "payment_id": payment_id,
        "amount_money": money_json(amount),
    });
    let reason = truncate_chars(request.reason.trim(), REFUND_REASON_MAX);
    if !reason.is_empty() {
        body["reason"] = Value::String(reason);
    }
    body
}

fn parse_payment(account: &SquareAccount, value: &Value) -> Result<PaymentIntent, PaymentError> {
    let payment = value
        .get("payment")
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            PaymentError::Serialization("square response has no payment object".to_string())
        })?;
    let id = string_field(payment, "id")
        .ok_or_else(|| PaymentError::Serialization("square payment has no id".to_string()))?;
    let provider_status = string_field(payment, "status").unwrap_or_default();
    let status = map_payment_status(provider_status);
    let amount = money_from(payment.get("amount_money")).ok_or_else(|| {
        PaymentError::Serialization("square payment has no amount_money".to_string())
    })?;
    let order_id = string_field(payment, "reference_id").unwrap_or_default();
    let mut intent = PaymentIntent::new(
        PROVIDER,
        account.id.clone(),
        order_id,
        amount,
        status,
        provider_status,
    );
    intent.provider_payment_id = Some(id.to_string());
    intent.created_at = string_field(payment, "created_at")
        .and_then(parse_timestamp)
        .unwrap_or(0);
    if status == PaymentStatus::Succeeded {
        intent.amount_captured = Some(amount);
    }
    intent.amount_refunded = money_from(payment.get("refunded_money"));
    Ok(intent)
}

fn parse_payment_link(
    account: &SquareAccount,
    value: &Value,
    request: &CreatePayment,
) -> Result<PaymentIntent, PaymentError> {
    let link = value
        .get("payment_link")
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            PaymentError::Serialization(
                "square payment link response has no payment_link object".to_string(),
            )
        })?;
    let url = string_field(link, "url")
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| PaymentError::Serialization("square payment link has no url".to_string()))?;
    // A payment link is not a payment object yet: the payer has to open the
    // hosted page, so the normalized state is `RequiresAction`.
    let mut intent = PaymentIntent::new(
        PROVIDER,
        account.id.clone(),
        request.order_id.clone(),
        request.amount,
        PaymentStatus::RequiresAction,
        "OPEN",
    );
    intent.client_action = Some(ClientAction::Redirect {
        url: url.to_string(),
    });
    intent.created_at = string_field(link, "created_at")
        .and_then(parse_timestamp)
        .unwrap_or_else(unix_now);
    if let Some(link_id) = string_field(link, "id") {
        intent
            .metadata
            .insert("square_payment_link_id".to_string(), link_id.to_string());
    }
    if let Some(order_id) = string_field(link, "order_id") {
        intent
            .metadata
            .insert("square_order_id".to_string(), order_id.to_string());
    }
    Ok(intent)
}

fn parse_refund(
    account: &SquareAccount,
    value: &Value,
    order_id: &str,
    refund_id: &str,
) -> Result<Refund, PaymentError> {
    let refund = value
        .get("refund")
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            PaymentError::Serialization("square response has no refund object".to_string())
        })?;
    let id = string_field(refund, "id")
        .ok_or_else(|| PaymentError::Serialization("square refund has no id".to_string()))?;
    let provider_status = string_field(refund, "status").unwrap_or_default();
    let status = map_refund_status(provider_status);
    let amount = money_from(refund.get("amount_money")).ok_or_else(|| {
        PaymentError::Serialization("square refund has no amount_money".to_string())
    })?;
    // The merchant order id is not echoed by the Refunds API; the create path
    // passes it in, the lookup path falls back to the Square order id.
    let resolved_order_id = if order_id.trim().is_empty() {
        string_field(refund, "order_id")
            .unwrap_or_default()
            .to_string()
    } else {
        order_id.to_string()
    };
    let resolved_refund_id = if refund_id.trim().is_empty() {
        id.to_string()
    } else {
        refund_id.to_string()
    };
    let succeeded_at = if status == RefundStatus::Succeeded {
        string_field(refund, "updated_at").and_then(parse_timestamp)
    } else {
        None
    };
    Ok(Refund {
        provider: PROVIDER,
        account: account.id.clone(),
        order_id: resolved_order_id,
        refund_id: resolved_refund_id,
        provider_refund_id: Some(id.to_string()),
        status,
        provider_status: provider_status.to_string(),
        amount,
        created_at: string_field(refund, "created_at")
            .and_then(parse_timestamp)
            .unwrap_or(0),
        succeeded_at,
        request_id: None,
    })
}

/// Register Square on a [`HostBuilder`].
pub trait SquareHostBuilderExt: Sized {
    /// Register the adapter with runtime configuration.
    ///
    /// Binds the `"Square"` configuration section through
    /// `add_options_monitor`, so appsettings/file reloads are picked up by the
    /// runtime cache (default 60s) or immediately after
    /// `Payments::invalidate`.
    fn add_square(self) -> Self;

    /// Register the adapter with a prebuilt static configuration.
    fn add_square_with(self, options: SquareOptions) -> Self;
}

impl SquareHostBuilderExt for HostBuilder {
    fn add_square(self) -> Self {
        let builder = self.add_options_monitor::<SquareOptions>("Square");
        builder.register(|services| {
            services
                .add_payment_provider_factory_lazy(|resolver| {
                    let monitor = resolver
                        .resolve_arc::<dyn webx_core::options_monitor::IOptionsMonitor<SquareOptions>>()
                        .expect("add_square() registers the Square options monitor");
                    Arc::new(SquareProviderFactory::new(monitor))
                })
                .add_payment_provider_builder(Arc::new(SquareProviderBuilder))
        })
    }

    fn add_square_with(self, options: SquareOptions) -> Self {
        let provider = Arc::new(SquareProvider::new(options));
        let health = Arc::clone(&provider);
        self.register(move |services| {
            services
                .add_payment_provider(provider)
                .add_payment_provider_builder(Arc::new(SquareProviderBuilder))
        })
        .add_health_check("payments.square", move || health.health())
    }
}

/// Runtime provider factory for Square (reads the options monitor).
pub struct SquareProviderFactory {
    monitor: Arc<dyn webx_core::options_monitor::IOptionsMonitor<SquareOptions>>,
}

impl SquareProviderFactory {
    /// Build the factory over an options monitor.
    pub fn new(
        monitor: Arc<dyn webx_core::options_monitor::IOptionsMonitor<SquareOptions>>,
    ) -> Self {
        Self { monitor }
    }
}

#[async_trait]
impl IPaymentProviderFactory for SquareProviderFactory {
    fn provider(&self) -> ProviderId {
        PROVIDER
    }

    async fn resolve(
        &self,
        _scope: &PaymentScope,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        let options = self.monitor.current();
        let provider = SquareProvider::new((*options).clone());
        provider.validate()?;
        Ok(Arc::new(provider))
    }
}

/// Builder used by custom `IPaymentServiceFactory` implementations.
#[derive(Debug, Default, Clone, Copy)]
pub struct SquareProviderBuilder;

impl IPaymentProviderBuilder for SquareProviderBuilder {
    fn provider(&self) -> ProviderId {
        PROVIDER
    }

    fn build(
        &self,
        config: &PaymentProviderConfig,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        let options: SquareOptions =
            serde_json::from_value(config.settings.clone()).map_err(|error| {
                PaymentError::config(
                    PROVIDER,
                    format!("invalid Square provider settings: {error}"),
                )
            })?;
        let provider = SquareProvider::new(options);
        provider.validate()?;
        Ok(Arc::new(provider))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webx_pay::Currency;

    use crate::options::{SquareAccountOptions, SquareEnvironment};

    fn account(location: Option<&str>) -> SquareAccount {
        SquareAccount {
            id: AccountId::from("default"),
            access_token: "test-access-token".to_string(),
            signature_key: Some("test-signature-key".to_string()),
            location_id: location.map(str::to_string),
            application_id: None,
            live: false,
            api_version: "2025-09-24".to_string(),
            base_url: SquareEnvironment::Sandbox.base_url().to_string(),
        }
    }

    fn card_request() -> CreatePayment {
        CreatePayment::new(
            "ORDER-1",
            Money::new(Currency::USD, 9900),
            PaymentMethod::Card,
            PaymentFlow::ClientSdk {
                return_url: Some("https://example.com/ok".to_string()),
            },
        )
        .with_description("Pro plan")
        .with_metadata(SOURCE_ID_METADATA, "test-source-token")
    }

    fn sandbox_options() -> SquareOptions {
        SquareOptions::empty().with_account(
            SquareAccountOptions::new("test-access-token")
                .with_environment(SquareEnvironment::Sandbox)
                .with_location_id("TESTLOCATION1"),
        )
    }

    fn scope() -> PaymentScope {
        PaymentScope::new(PROVIDER)
    }

    #[test]
    fn builds_the_create_payment_body() {
        let body = build_payment_body(&account(Some("TESTLOCATION1")), &card_request()).unwrap();
        assert_eq!(body["idempotency_key"], "ORDER-1");
        assert_eq!(body["location_id"], "TESTLOCATION1");
        assert_eq!(body["source_id"], "test-source-token");
        assert_eq!(body["amount_money"]["amount"], 9900);
        assert_eq!(body["amount_money"]["currency"], "USD");
        assert_eq!(body["reference_id"], "ORDER-1");
        assert_eq!(body["autocomplete"], true);
        assert_eq!(body["note"], "Pro plan");

        let manual = build_payment_body(
            &account(Some("TESTLOCATION1")),
            &card_request().with_manual_capture(),
        )
        .unwrap();
        assert_eq!(manual["autocomplete"], false);
    }

    #[test]
    fn create_payment_requires_a_client_side_token() {
        let request = CreatePayment::new(
            "ORDER-1",
            Money::new(Currency::USD, 9900),
            PaymentMethod::Card,
            PaymentFlow::ClientSdk { return_url: None },
        );
        let error = build_payment_body(&account(Some("TESTLOCATION1")), &request).unwrap_err();
        assert_eq!(error.http_status(), 400);
        assert!(error.to_string().contains("square_source_id"));
    }

    #[test]
    fn create_payment_requires_a_location_and_bounded_reference() {
        let error = build_payment_body(&account(None), &card_request()).unwrap_err();
        assert!(error.to_string().contains("location_id"));

        let long_order = CreatePayment::new(
            "O".repeat(41),
            Money::new(Currency::USD, 100),
            PaymentMethod::Card,
            PaymentFlow::ClientSdk { return_url: None },
        )
        .with_metadata(SOURCE_ID_METADATA, "test-source-token");
        let error = build_payment_body(&account(Some("TESTLOCATION1")), &long_order).unwrap_err();
        assert!(error.to_string().contains("reference_id"));
    }

    #[test]
    fn rejects_unsupported_payment_methods() {
        let request = CreatePayment::new(
            "ORDER-1",
            Money::new(Currency::USD, 100),
            PaymentMethod::AlipayPage,
            PaymentFlow::ClientSdk { return_url: None },
        )
        .with_metadata(SOURCE_ID_METADATA, "test-source-token");
        let error = build_payment_body(&account(Some("TESTLOCATION1")), &request).unwrap_err();
        assert!(matches!(
            error,
            PaymentError::Unsupported {
                capability: "payment_method",
                ..
            }
        ));
    }

    #[test]
    fn builds_the_payment_link_body() {
        let mut request = card_request();
        request.capture = CaptureMode::Automatic;
        let body = build_payment_link_body(
            &account(Some("TESTLOCATION1")),
            &request,
            "https://example.com/ok",
        )
        .unwrap();
        assert_eq!(body["idempotency_key"], "ORDER-1");
        assert_eq!(body["order"]["location_id"], "TESTLOCATION1");
        assert_eq!(body["order"]["reference_id"], "ORDER-1");
        assert_eq!(body["order"]["line_items"][0]["name"], "Pro plan");
        assert_eq!(body["order"]["line_items"][0]["quantity"], "1");
        assert_eq!(
            body["order"]["line_items"][0]["base_price_money"]["amount"],
            9900
        );
        assert_eq!(
            body["checkout_options"]["redirect_url"],
            "https://example.com/ok"
        );

        let error = build_payment_link_body(
            &account(Some("TESTLOCATION1")),
            &card_request().with_manual_capture(),
            "https://example.com/ok",
        )
        .unwrap_err();
        assert!(error.to_string().contains("manual capture"));

        let error = build_payment_link_body(&account(Some("TESTLOCATION1")), &card_request(), " ")
            .unwrap_err();
        assert!(error.to_string().contains("success_url"));
    }

    #[test]
    fn builds_the_refund_body() {
        let request = RefundCommand::new("PAYMENT-1", "REFUND-1")
            .with_amount(Money::new(Currency::USD, 500))
            .with_reason("customer request");
        let body = build_refund_body(&request, "PAYMENT-1", &Money::new(Currency::USD, 500));
        assert_eq!(body["idempotency_key"], "REFUND-1");
        assert_eq!(body["payment_id"], "PAYMENT-1");
        assert_eq!(body["amount_money"]["amount"], 500);
        assert_eq!(body["reason"], "customer request");
    }

    #[test]
    fn provider_ids_must_look_like_square_ids() {
        assert!(payment_id_arg("ORDER-1").is_err());
        assert!(payment_id_arg("R2B3Z8WMVt3EAmzYWLZvz7Y69EbZY").is_ok());
        assert!(
            refund_id_arg("R2B3Z8WMVt3EAmzYWLZvz7Y69EbZY_KlWP8IC1557ddwc9QWTKrCVU6m0JXDz").is_ok()
        );
        assert!(refund_id_arg("REFUND-1").is_err());
        // Path characters never reach the URL.
        assert!(payment_id_arg("../../v2/payments/x").is_err());
    }

    #[test]
    fn parses_a_square_payment_object() {
        let value = json!({
            "payment": {
                "id": "R2B3Z8WMVt3EAmzYWLZvz7Y69EbZY",
                "status": "COMPLETED",
                "amount_money": { "amount": 9900, "currency": "USD" },
                "refunded_money": { "amount": 100, "currency": "USD" },
                "reference_id": "ORDER-1",
                "created_at": "2021-10-13T21:14:29.577Z"
            }
        });
        let intent = parse_payment(&account(Some("TESTLOCATION1")), &value).unwrap();
        assert_eq!(intent.provider, PROVIDER);
        assert_eq!(intent.status, PaymentStatus::Succeeded);
        assert_eq!(intent.provider_status, "COMPLETED");
        assert_eq!(intent.order_id, "ORDER-1");
        assert_eq!(
            intent.provider_payment_id.as_deref(),
            Some("R2B3Z8WMVt3EAmzYWLZvz7Y69EbZY")
        );
        assert_eq!(intent.amount, Money::new(Currency::USD, 9900));
        assert_eq!(
            intent.amount_captured,
            Some(Money::new(Currency::USD, 9900))
        );
        assert_eq!(intent.amount_refunded, Some(Money::new(Currency::USD, 100)));
        assert_eq!(intent.created_at, 1_634_159_669);
    }

    #[test]
    fn parses_a_square_payment_link() {
        let value = json!({
            "payment_link": {
                "id": "LINK-1",
                "version": 1,
                "order_id": "SQUARE-ORDER-1",
                "url": "https://square.link/u/EXAMPLE",
                "created_at": "2021-10-13T21:14:29.577Z"
            }
        });
        let intent =
            parse_payment_link(&account(Some("TESTLOCATION1")), &value, &card_request()).unwrap();
        assert_eq!(intent.status, PaymentStatus::RequiresAction);
        assert_eq!(
            intent.client_action,
            Some(ClientAction::Redirect {
                url: "https://square.link/u/EXAMPLE".to_string()
            })
        );
        assert_eq!(
            intent.metadata.get("square_order_id").map(String::as_str),
            Some("SQUARE-ORDER-1")
        );
    }

    #[test]
    fn parses_a_square_refund() {
        let value = json!({
            "refund": {
                "id": "PAYMENT-1_REFUND-1",
                "payment_id": "PAYMENT-1",
                "order_id": "SQUARE-ORDER-1",
                "status": "APPROVED",
                "amount_money": { "amount": 500, "currency": "USD" },
                "created_at": "2021-10-13T21:23:19.116Z",
                "updated_at": "2021-10-13T21:23:19.508Z"
            }
        });
        let refund = parse_refund(
            &account(Some("TESTLOCATION1")),
            &value,
            "PAYMENT-1",
            "REFUND-1",
        )
        .unwrap();
        assert_eq!(refund.status, RefundStatus::Succeeded);
        assert_eq!(refund.order_id, "PAYMENT-1");
        assert_eq!(refund.refund_id, "REFUND-1");
        assert_eq!(
            refund.provider_refund_id.as_deref(),
            Some("PAYMENT-1_REFUND-1")
        );
        assert_eq!(refund.amount, Money::new(Currency::USD, 500));
        assert_eq!(refund.succeeded_at, Some(1_634_160_199));
    }

    #[test]
    fn capabilities_match_the_implemented_operations() {
        let provider = SquareProvider::new(sandbox_options());
        let capabilities = provider.capabilities();
        assert!(capabilities.create);
        assert!(capabilities.query);
        assert!(capabilities.capture);
        assert!(capabilities.cancel);
        assert!(capabilities.refund);
        assert!(capabilities.partial_refund);
        assert!(capabilities.hosted_checkout);
        assert!(!capabilities.qr);
        assert!(!capabilities.app);
        assert_eq!(capabilities.webhook, WebhookCapability::HmacSha256);
        assert!(capabilities.supports_currency(&Currency::USD));
        assert_eq!(provider.id(), ProviderId::Square);
        assert_eq!(provider.accounts().len(), 1);
        assert!(!provider.accounts()[0].live);
    }

    #[test]
    fn payment_options_expose_public_card_client_sdk_config() {
        let provider = SquareProvider::new(
            SquareOptions::empty()
                .with_account(
                    SquareAccountOptions::new("test-access-token")
                        .with_environment(SquareEnvironment::Sandbox)
                        .with_application_id("test-application-id")
                        .with_location_id("TESTLOCATION1"),
                )
                .with_account(
                    SquareAccountOptions::new("live-access-token")
                        .with_id("live")
                        .with_application_id("live-application-id"),
                ),
        );
        let options = provider.payment_options();
        assert_eq!(options.len(), 2);

        let sandbox = options
            .iter()
            .find(|option| option.account.as_str() == "default")
            .expect("default account option");
        assert_eq!(sandbox.provider, PROVIDER);
        assert_eq!(sandbox.method, PaymentMethod::Card);
        assert_eq!(sandbox.flows, vec![PaymentFlowKind::ClientSdk]);
        assert!(sandbox.label.is_none());
        let config = sandbox.client_config.as_ref().expect("client config");
        assert_eq!(config["application_id"], "test-application-id");
        assert_eq!(config["location_id"], "TESTLOCATION1");
        assert_eq!(config["environment"], "sandbox");

        let live = options
            .iter()
            .find(|option| option.account.as_str() == "live")
            .expect("live account option");
        let config = live.client_config.as_ref().expect("client config");
        assert_eq!(config["application_id"], "live-application-id");
        assert_eq!(config["environment"], "production");
        // Public fields that are not configured are omitted, not null.
        assert!(config.get("location_id").is_none());
    }

    #[test]
    fn payment_options_omit_an_unconfigured_application_id() {
        let provider = SquareProvider::new(sandbox_options());
        let options = provider.payment_options();
        assert_eq!(options.len(), 1);
        let config = options[0].client_config.as_ref().expect("client config");
        assert!(config.get("application_id").is_none());
        assert_eq!(config["location_id"], "TESTLOCATION1");
        assert_eq!(config["environment"], "sandbox");
    }

    #[test]
    fn payment_options_never_serialize_credentials() {
        let provider = SquareProvider::new(
            SquareOptions::empty()
                .with_account(
                    SquareAccountOptions::new("square-access-token-secret")
                        .with_environment(SquareEnvironment::Sandbox)
                        .with_application_id("test-application-id")
                        .with_location_id("TESTLOCATION1")
                        .with_signature_key("square-signature-key-secret"),
                )
                .with_webhook_url("https://example.com/webhooks/payments/square"),
        );
        let json = serde_json::to_string(&provider.payment_options()).expect("catalog JSON");
        for secret in ["square-access-token-secret", "square-signature-key-secret"] {
            assert!(!json.contains(secret), "catalog leaked `{secret}`");
        }
        assert!(!json.contains("access_token"));
        assert!(!json.contains("signature_key"));
        assert!(json.contains("test-application-id"));

        let value: Value = serde_json::from_str(&json).expect("catalog round-trips");
        let mut keys: Vec<&str> = value[0]["client_config"]
            .as_object()
            .expect("client config object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["application_id", "environment", "location_id"]);
    }

    #[test]
    fn payment_options_are_empty_when_the_configuration_is_invalid() {
        let provider = SquareProvider::new(SquareOptions::empty());
        assert!(provider.payment_options().is_empty());
    }

    #[test]
    fn validate_checks_configuration() {
        let provider = SquareProvider::new(sandbox_options());
        assert!(provider.validate().is_ok());
        assert_eq!(provider.health().status, "pass");

        let broken = SquareProvider::new(SquareOptions::empty());
        let error = broken.validate().unwrap_err();
        assert_eq!(error.http_status(), 500);
        assert!(error.to_string().contains("at least one account"));
        assert_eq!(broken.health().status, "fail");
    }

    #[tokio::test]
    async fn create_payment_without_a_token_fails_before_any_http_call() {
        let provider = SquareProvider::new(sandbox_options());
        let request = CreatePayment::new(
            "ORDER-1",
            Money::new(Currency::USD, 9900),
            PaymentMethod::Card,
            PaymentFlow::ClientSdk { return_url: None },
        );
        let error = provider
            .create_payment(&scope(), request)
            .await
            .unwrap_err();
        assert_eq!(error.http_status(), 400);
        assert!(error.to_string().contains("square_source_id"));
    }

    #[tokio::test]
    async fn qr_and_app_flows_are_unsupported() {
        let provider = SquareProvider::new(sandbox_options());
        let qr = CreatePayment::new(
            "ORDER-1",
            Money::new(Currency::USD, 9900),
            PaymentMethod::Card,
            PaymentFlow::QrCode,
        );
        assert!(matches!(
            provider.create_payment(&scope(), qr).await.unwrap_err(),
            PaymentError::Unsupported {
                capability: "flow.qr_code",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn delayed_capture_rejects_partial_amounts() {
        let provider = SquareProvider::new(sandbox_options());
        let error = provider
            .capture_payment(
                &scope(),
                "R2B3Z8WMVt3EAmzYWLZvz7Y69EbZY",
                Some(Money::new(Currency::USD, 100)),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("partial capture"));
    }

    #[tokio::test]
    async fn lookup_accepts_only_square_payment_ids() {
        let provider = SquareProvider::new(sandbox_options());
        let error = provider.get_payment(&scope(), "ORDER-1").await.unwrap_err();
        assert_eq!(error.http_status(), 400);
        assert!(error
            .to_string()
            .contains("no lookup by merchant order reference"));

        let error = provider.get_refund(&scope(), "REFUND-1").await.unwrap_err();
        assert_eq!(error.http_status(), 400);
    }

    #[test]
    fn builder_parses_settings_json_and_validates() {
        let config = PaymentProviderConfig::new(
            PROVIDER,
            json!({
                "accounts": [{ "access_token": "test-access-token" }],
                "webhook_url": "https://example.com/webhooks/payments/square"
            }),
        );
        let provider = SquareProviderBuilder.build(&config).unwrap();
        assert_eq!(provider.id(), ProviderId::Square);
        assert_eq!(provider.accounts().len(), 1);

        let invalid = PaymentProviderConfig::new(PROVIDER, json!({}));
        let error = SquareProviderBuilder
            .build(&invalid)
            .err()
            .expect("empty Square settings must not build");
        assert_eq!(error.http_status(), 500);
        assert!(error.to_string().contains("at least one account"));
    }

    #[tokio::test]
    async fn factory_resolves_the_current_monitor_snapshot() {
        let monitor = Arc::new(webx_core::options_monitor::OptionsMonitor::new(
            sandbox_options(),
        ));
        let factory = SquareProviderFactory::new(monitor);
        assert_eq!(factory.provider(), ProviderId::Square);
        let provider = factory.resolve(&scope()).await.unwrap();
        assert_eq!(provider.id(), ProviderId::Square);
        assert_eq!(provider.accounts().len(), 1);
    }
}
