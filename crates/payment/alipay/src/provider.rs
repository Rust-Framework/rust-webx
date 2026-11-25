//! Alipay provider, gateway calls and host registration.
//!
//! Supported operations (official gateway methods):
//!
//! | capability | method |
//! |---|---|
//! | page (desktop web redirect) | `alipay.trade.page.pay` |
//! | wap (mobile web redirect) | `alipay.trade.wap.pay` |
//! | app SDK order string | `alipay.trade.app.pay` |
//! | QR code | `alipay.trade.precreate` |
//! | query | `alipay.trade.query` |
//! | close | `alipay.trade.close` |
//! | refund | `alipay.trade.refund` (`out_request_no` = merchant refund id) |
//! | refund query | `alipay.trade.fastpay.refund.query` |
//!
//! Alipay has no manual capture, so `capture_payment` returns
//! [`PaymentError::Unsupported`]. Amounts are CNY only and always sent as the
//! two-decimal yuan string.
//!
//! # Frontend catalog
//!
//! [`IPaymentProvider::payment_options`] lists, for every configured account,
//! the four enabled channels: page and WAP redirects, the app SDK order string
//! and QR `precreate`. Each option carries only the public `app_id` in
//! `client_config`; the application private key, Alipay public key and
//! certificates never leave the adapter.
//!
//! # Merchant ids
//!
//! * `CreatePayment::order_id` is sent as `out_trade_no` (Alipay's creation
//!   idempotency anchor).
//! * `RefundCommand::refund_id` is sent as `out_request_no` (Alipay's refund
//!   idempotency anchor), so refunds with the same merchant refund id never
//!   refund twice.
//! * `get_refund` takes `out_trade_no:out_request_no`, because
//!   `alipay.trade.fastpay.refund.query` identifies a refund by both values.
//! * `CreatePayment::metadata` is echoed on the returned [`PaymentIntent`] but
//!   is not forwarded to Alipay (`passback_params` is intentionally not used).

use std::fmt;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use bytes::Bytes;
use rsa::{PrivateKey, PublicKey};
use rust_dix::IServiceResolverExt;
use serde_json::Value;
use webx_client::{HttpClient, IHttpClient, OutboundRequest};
use webx_host::health::HealthCheckSpec;
use webx_host::health::HealthStatus;
use webx_host::server::HostBuilder;
use webx_pay::{
    AccountId, AccountSet, CaptureMode, ClientAction, CreatePayment, Currency, CurrencyScope,
    IPaymentProvider, IPaymentProviderBuilder, IPaymentProviderFactory, IPaymentWebhookVerifier,
    PaymentAccount, PaymentError, PaymentFlow, PaymentFlowKind, PaymentIntent, PaymentMethod,
    PaymentOption, PaymentProviderConfig, PaymentScope, PaymentStatus,
    PaymentsServiceCollectionExt, ProviderCapabilities, ProviderId, Refund, RefundCommand,
    RefundStatus, WebhookCapability,
};

use crate::mapping::{
    cny_amount_string, map_refund_status, map_trade_status, money_cny, string_field,
};
use crate::options::AlipayOptions;
use crate::time;
use crate::transport;
use crate::webhook::AlipayWebhookVerifier;
use crate::PROVIDER;

/// Product code of `alipay.trade.page.pay`.
const PRODUCT_PAGE_PAY: &str = "FAST_INSTANT_TRADE_PAY";
/// Product code of `alipay.trade.wap.pay`.
const PRODUCT_WAP_PAY: &str = "QUICK_WAP_WAY";
/// Product code of `alipay.trade.app.pay`.
const PRODUCT_APP_PAY: &str = "QUICK_MSECURITY_PAY";
/// Product code of `alipay.trade.precreate`.
const PRODUCT_QR_PAY: &str = "FACE_TO_FACE_PAYMENT";

/// Alipay's status for a trade that exists but has not been paid.
const STATUS_WAIT_BUYER_PAY: &str = "WAIT_BUYER_PAY";
/// Alipay's status for a closed trade.
const STATUS_TRADE_CLOSED: &str = "TRADE_CLOSED";

/// Credentials and parsed keys for one Alipay merchant account.
#[derive(Clone)]
pub(crate) struct AlipayAccount {
    pub(crate) id: AccountId,
    pub(crate) app_id: String,
    pub(crate) private_key: PrivateKey,
    pub(crate) public_key: PublicKey,
    pub(crate) certificate_mode: bool,
    pub(crate) app_cert_sn: Option<String>,
    pub(crate) alipay_root_cert_sn: Option<String>,
    pub(crate) notify_url: Option<String>,
    pub(crate) return_url: Option<String>,
    pub(crate) live: bool,
}

impl fmt::Debug for AlipayAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlipayAccount")
            .field("id", &self.id)
            .field("app_id", &self.app_id)
            .field("certificate_mode", &self.certificate_mode)
            .field("private_key", &"<redacted>")
            .field("public_key", &"<redacted>")
            .field("app_cert_sn", &self.app_cert_sn)
            .field("alipay_root_cert_sn", &self.alipay_root_cert_sn)
            .field("notify_url", &self.notify_url)
            .field("return_url", &self.return_url)
            .field("live", &self.live)
            .finish()
    }
}

/// Result of one gateway call.
struct GatewayReply {
    node: Value,
    request_id: Option<String>,
}

/// Alipay adapter.
pub struct AlipayProvider {
    options: AlipayOptions,
    accounts: Result<AccountSet<AlipayAccount>, String>,
    verifier: AlipayWebhookVerifier,
    injected_client: Option<Arc<dyn IHttpClient>>,
    client: OnceLock<Result<Arc<dyn IHttpClient>, String>>,
    validated: OnceLock<Result<(), String>>,
}

impl AlipayProvider {
    /// Build a provider from options (configuration is validated at startup).
    pub fn new(options: AlipayOptions) -> Self {
        let accounts = options.build_accounts();
        let verifier = AlipayWebhookVerifier::new(accounts.clone());
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
    pub fn with_client(options: AlipayOptions, client: Arc<dyn IHttpClient>) -> Self {
        let mut provider = Self::new(options);
        provider.injected_client = Some(client);
        provider
    }

    fn account_set(&self) -> Result<&AccountSet<AlipayAccount>, PaymentError> {
        self.accounts
            .as_ref()
            .map_err(|error| PaymentError::config(PROVIDER, error.clone()))
    }

    fn account_for(
        &self,
        requested: Option<&AccountId>,
    ) -> Result<&Arc<AlipayAccount>, PaymentError> {
        self.account_set()?.resolve(requested)
    }

    fn gateway(&self) -> Result<String, PaymentError> {
        self.options
            .gateway_url()
            .map_err(|detail| PaymentError::config(PROVIDER, detail))
    }

    fn client(&self) -> Result<Arc<dyn IHttpClient>, PaymentError> {
        if let Some(client) = &self.injected_client {
            return Ok(Arc::clone(client));
        }
        let built = self.client.get_or_init(|| {
            let gateway = self.options.gateway_url()?;
            HttpClient::new("alipay", self.options.http_options(&gateway))
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

    /// Execute one gateway method and return its verified response node.
    async fn post_node(
        &self,
        account: &AlipayAccount,
        method: &str,
        biz_content: &Value,
        notify_url: Option<&str>,
    ) -> Result<GatewayReply, PaymentError> {
        let gateway = self.gateway()?;
        let params = transport::signed_params(
            account,
            method,
            biz_content,
            notify_url,
            None,
            time::unix_now(),
        )?;
        let form = transport::encode_form(&params);
        let client = self.client()?;
        let request = OutboundRequest::post(gateway)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "application/json")
            .body(Bytes::from(form));
        let response = client.execute(request).await?;
        let status = response.status;
        let request_id = response
            .headers
            .get("trace_id")
            .or_else(|| response.headers.get("trace-id"))
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = response.body.collect(transport::MAX_RESPONSE_BYTES).await?;
        if !(200..300).contains(&status) {
            return Err(PaymentError::provider(
                PROVIDER,
                None,
                format!("alipay gateway returned HTTP {status}"),
                Some(status),
                request_id,
                status == 429 || status >= 500,
            ));
        }
        let raw = std::str::from_utf8(&body).map_err(|error| {
            PaymentError::Serialization(format!("alipay response is not UTF-8: {error}"))
        })?;
        let (is_error, node) = transport::parse_response(raw, method, &account.public_key)?;
        let node = transport::ensure_success(is_error, node, request_id.clone())?;
        Ok(GatewayReply { node, request_id })
    }

    async fn create_payment_inner(
        &self,
        scope: &PaymentScope,
        request: CreatePayment,
    ) -> Result<PaymentIntent, PaymentError> {
        request.validate()?;
        if request.amount.currency != Currency::CNY {
            return Err(PaymentError::validation(format!(
                "alipay supports CNY amounts only (got {})",
                request.amount.currency
            )));
        }
        if request.capture == CaptureMode::Manual {
            return Err(PaymentError::unsupported(PROVIDER, "capture"));
        }
        let account = self.account_for(scope.account_ref())?;
        let now = time::unix_now();
        let notify_url = request
            .notify_url
            .clone()
            .or_else(|| account.notify_url.clone());
        match &request.method {
            PaymentMethod::AlipayPage => {
                ensure_redirect_flow(&request.flow)?;
                let return_url = return_url_for(account, &request);
                validate_web_url(return_url, "return_url")?;
                let biz = create_biz(&request, PRODUCT_PAGE_PAY)?;
                let params = transport::signed_params(
                    account,
                    transport::METHOD_PAGE_PAY,
                    &biz,
                    notify_url.as_deref(),
                    return_url,
                    now,
                )?;
                let mut intent = base_intent(
                    account,
                    &request,
                    PaymentStatus::RequiresAction,
                    STATUS_WAIT_BUYER_PAY,
                    now,
                );
                intent.client_action = Some(ClientAction::Redirect {
                    url: transport::redirect_url(&self.gateway()?, &params),
                });
                Ok(intent)
            }
            PaymentMethod::AlipayWap => {
                ensure_redirect_flow(&request.flow)?;
                let return_url = return_url_for(account, &request);
                validate_web_url(return_url, "return_url")?;
                let mut biz = create_biz(&request, PRODUCT_WAP_PAY)?;
                if let PaymentFlow::Redirect {
                    cancel_url: Some(cancel_url),
                    ..
                } = &request.flow
                {
                    if !cancel_url.trim().is_empty() {
                        validate_web_url(Some(cancel_url), "cancel_url")?;
                        biz["quit_url"] = Value::String(cancel_url.clone());
                    }
                }
                let params = transport::signed_params(
                    account,
                    transport::METHOD_WAP_PAY,
                    &biz,
                    notify_url.as_deref(),
                    return_url,
                    now,
                )?;
                let mut intent = base_intent(
                    account,
                    &request,
                    PaymentStatus::RequiresAction,
                    STATUS_WAIT_BUYER_PAY,
                    now,
                );
                intent.client_action = Some(ClientAction::Redirect {
                    url: transport::redirect_url(&self.gateway()?, &params),
                });
                Ok(intent)
            }
            PaymentMethod::AlipayApp => {
                ensure_app_flow(&request.flow)?;
                let biz = create_biz(&request, PRODUCT_APP_PAY)?;
                let params = transport::signed_params(
                    account,
                    transport::METHOD_APP_PAY,
                    &biz,
                    notify_url.as_deref(),
                    None,
                    now,
                )?;
                let order_str = transport::encode_form(&params);
                let mut intent = base_intent(
                    account,
                    &request,
                    PaymentStatus::RequiresAction,
                    STATUS_WAIT_BUYER_PAY,
                    now,
                );
                intent.client_action = Some(ClientAction::AppOrder {
                    provider: PROVIDER,
                    payload: serde_json::json!({ "order_str": order_str }),
                });
                Ok(intent)
            }
            PaymentMethod::AlipayQr => {
                ensure_qr_flow(&request.flow)?;
                let biz = create_biz(&request, PRODUCT_QR_PAY)?;
                let reply = self
                    .post_node(
                        account,
                        transport::METHOD_PRECREATE,
                        &biz,
                        notify_url.as_deref(),
                    )
                    .await?;
                let qr_code = string_field(&reply.node, "qr_code").ok_or_else(|| {
                    PaymentError::Serialization(
                        "alipay precreate response has no qr_code".to_string(),
                    )
                })?;
                let mut intent = base_intent(
                    account,
                    &request,
                    PaymentStatus::RequiresAction,
                    STATUS_WAIT_BUYER_PAY,
                    now,
                );
                intent.request_id = reply.request_id;
                intent.client_action = Some(ClientAction::QrCode { content: qr_code });
                Ok(intent)
            }
            _ => Err(PaymentError::unsupported(PROVIDER, "payment_method")),
        }
    }

    async fn get_payment_inner(
        &self,
        account: &AlipayAccount,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        let reply = self
            .post_node(
                account,
                transport::METHOD_TRADE_QUERY,
                &serde_json::json!({ "out_trade_no": order_id }),
                None,
            )
            .await?;
        intent_from_query(&reply, account, order_id)
    }

    async fn cancel_payment_inner(
        &self,
        scope: &PaymentScope,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        let order_id = validate_order_id(order_id)?;
        let account = self.account_for(scope.account_ref())?;
        let current = self.get_payment_inner(account, order_id).await?;
        if current.status == PaymentStatus::Canceled {
            return Ok(current);
        }
        if current.status.is_paid() {
            return Err(PaymentError::conflict(
                PROVIDER,
                format!("alipay trade `{order_id}` is already paid and cannot be closed"),
            ));
        }
        let reply = self
            .post_node(
                account,
                transport::METHOD_TRADE_CLOSE,
                &serde_json::json!({ "out_trade_no": order_id }),
                None,
            )
            .await?;
        let mut intent = current;
        intent.status = PaymentStatus::Canceled;
        intent.provider_status = STATUS_TRADE_CLOSED.to_string();
        if let Some(trade_no) = string_field(&reply.node, "trade_no") {
            intent.provider_payment_id = Some(trade_no);
        }
        intent.client_action = None;
        intent.request_id = reply.request_id;
        Ok(intent)
    }

    async fn refund_inner(
        &self,
        scope: &PaymentScope,
        request: RefundCommand,
    ) -> Result<Refund, PaymentError> {
        request.validate()?;
        let account = self.account_for(scope.account_ref())?;
        let refund_amount = request.amount.as_ref().map(cny_amount_string).transpose()?;
        let mut biz = serde_json::json!({
            "out_trade_no": request.order_id,
            // `out_request_no` is Alipay's idempotency anchor: repeating the
            // same refund id never refunds twice.
            "out_request_no": request.refund_id,
        });
        if let Some(refund_amount) = refund_amount {
            biz["refund_amount"] = Value::String(refund_amount);
        }
        if !request.reason.trim().is_empty() {
            biz["refund_reason"] = Value::String(request.reason.clone());
        }
        let notify_url = request
            .notify_url
            .clone()
            .or_else(|| account.notify_url.clone());
        let reply = self
            .post_node(
                account,
                transport::METHOD_TRADE_REFUND,
                &biz,
                notify_url.as_deref(),
            )
            .await?;
        let fund_change = string_field(&reply.node, "fund_change");
        let status = match fund_change.as_deref() {
            Some("Y") => RefundStatus::Succeeded,
            Some("N") => RefundStatus::Failed,
            _ => RefundStatus::Pending,
        };
        let amount = match &request.amount {
            Some(amount) => *amount,
            None => string_field(&reply.node, "refund_fee")
                .and_then(|value| money_cny(&value))
                .ok_or_else(|| {
                    PaymentError::Serialization(
                        "alipay refund response has no refund_fee".to_string(),
                    )
                })?,
        };
        let succeeded_at = if status == RefundStatus::Succeeded {
            string_field(&reply.node, "gmt_refund_pay")
                .and_then(|value| time::parse_datetime(&value))
        } else {
            None
        };
        Ok(Refund {
            provider: PROVIDER,
            account: account.id.clone(),
            order_id: request.order_id,
            refund_id: request.refund_id,
            provider_refund_id: None,
            status,
            provider_status: fund_change.unwrap_or_default(),
            amount,
            created_at: 0,
            succeeded_at,
            request_id: reply.request_id,
        })
    }

    async fn get_refund_inner(
        &self,
        scope: &PaymentScope,
        refund_id: &str,
    ) -> Result<Refund, PaymentError> {
        let (order_id, out_request_no) = split_refund_lookup_id(refund_id)?;
        let account = self.account_for(scope.account_ref())?;
        let reply = self
            .post_node(
                account,
                transport::METHOD_REFUND_QUERY,
                &serde_json::json!({
                    "out_trade_no": order_id,
                    "out_request_no": out_request_no,
                }),
                None,
            )
            .await?;
        let status_raw = string_field(&reply.node, "refund_status");
        let amount = string_field(&reply.node, "refund_amount")
            .and_then(|value| money_cny(&value))
            .ok_or_else(|| {
                PaymentError::Serialization(
                    "alipay refund query response has no refund_amount".to_string(),
                )
            })?;
        Ok(Refund {
            provider: PROVIDER,
            account: account.id.clone(),
            order_id: string_field(&reply.node, "out_trade_no")
                .unwrap_or_else(|| order_id.to_string()),
            refund_id: string_field(&reply.node, "out_request_no")
                .unwrap_or_else(|| out_request_no.to_string()),
            provider_refund_id: None,
            status: map_refund_status(status_raw.as_deref()),
            provider_status: status_raw.unwrap_or_default(),
            amount,
            created_at: 0,
            succeeded_at: None,
            request_id: reply.request_id,
        })
    }
}

#[async_trait]
impl IPaymentProvider for AlipayProvider {
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
            capture: false,
            cancel: true,
            refund: true,
            partial_refund: true,
            hosted_checkout: true,
            qr: true,
            app: true,
            webhook: WebhookCapability::Rsa2,
            currencies: CurrencyScope::Only(&["CNY"]),
            statuses: &[
                PaymentStatus::RequiresAction,
                PaymentStatus::Succeeded,
                PaymentStatus::Canceled,
            ],
        }
    }

    fn payment_options(&self) -> Vec<PaymentOption> {
        let Ok(accounts) = self.account_set() else {
            return Vec::new();
        };
        let public = accounts.accounts();
        let mut options = Vec::with_capacity(accounts.len() * 4);
        for (id, account) in accounts.iter() {
            let client_config = serde_json::json!({ "app_id": account.app_id });
            let label = public
                .iter()
                .find(|info| &info.id == id)
                .and_then(|info| info.label.clone());
            for (method, flow) in [
                (PaymentMethod::AlipayPage, PaymentFlowKind::Redirect),
                (PaymentMethod::AlipayWap, PaymentFlowKind::Redirect),
                (PaymentMethod::AlipayApp, PaymentFlowKind::App),
                (PaymentMethod::AlipayQr, PaymentFlowKind::QrCode),
            ] {
                let mut option = PaymentOption::new(PROVIDER, id.clone(), method, [flow])
                    .with_client_config(client_config.clone());
                if let Some(label) = &label {
                    option = option.with_label(label.clone());
                }
                options.push(option);
            }
        }
        options
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
        self.create_payment_inner(scope, request).await
    }

    async fn get_payment(
        &self,
        scope: &PaymentScope,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        let order_id = validate_order_id(order_id)?;
        let account = self.account_for(scope.account_ref())?;
        self.get_payment_inner(account, order_id).await
    }

    async fn cancel_payment(
        &self,
        scope: &PaymentScope,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        self.cancel_payment_inner(scope, order_id).await
    }

    async fn refund(
        &self,
        scope: &PaymentScope,
        request: RefundCommand,
    ) -> Result<Refund, PaymentError> {
        self.refund_inner(scope, request).await
    }

    async fn get_refund(
        &self,
        scope: &PaymentScope,
        refund_id: &str,
    ) -> Result<Refund, PaymentError> {
        self.get_refund_inner(scope, refund_id).await
    }

    fn webhook_verifier(&self) -> &dyn IPaymentWebhookVerifier {
        &self.verifier
    }
}

fn base_intent(
    account: &AlipayAccount,
    request: &CreatePayment,
    status: PaymentStatus,
    provider_status: &str,
    now: i64,
) -> PaymentIntent {
    let mut intent = PaymentIntent::new(
        PROVIDER,
        account.id.clone(),
        request.order_id.clone(),
        request.amount,
        status,
        provider_status,
    );
    intent.created_at = now;
    intent.expires_at = request.expires_at;
    intent.metadata = request.metadata.clone();
    intent
}

fn create_biz(request: &CreatePayment, product_code: &str) -> Result<Value, PaymentError> {
    let subject = if request.description.trim().is_empty() {
        format!("Order {}", request.order_id)
    } else {
        request.description.clone()
    };
    let mut biz = serde_json::json!({
        "out_trade_no": request.order_id,
        "total_amount": cny_amount_string(&request.amount)?,
        "subject": subject,
        "product_code": product_code,
    });
    if let Some(expires_at) = request.expires_at {
        biz["time_expire"] = Value::String(time::format_datetime(expires_at));
    }
    Ok(biz)
}

fn return_url_for<'a>(account: &'a AlipayAccount, request: &'a CreatePayment) -> Option<&'a str> {
    match &request.flow {
        PaymentFlow::Redirect { success_url, .. } => {
            Some(success_url.as_str()).filter(|url| !url.trim().is_empty())
        }
        PaymentFlow::ClientSdk { return_url } => return_url
            .as_deref()
            .filter(|url| !url.trim().is_empty())
            .or(account.return_url.as_deref()),
        PaymentFlow::App { return_url } => return_url
            .as_deref()
            .filter(|url| !url.trim().is_empty())
            .or(account.return_url.as_deref()),
        PaymentFlow::QrCode => None,
    }
}

fn validate_web_url(value: Option<&str>, field: &str) -> Result<(), PaymentError> {
    let Some(value) = value else {
        return Ok(());
    };
    let parsed = url::Url::parse(value).map_err(|error| {
        PaymentError::validation(format!("{field} is not a valid URL: {error}"))
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(PaymentError::validation(format!(
            "{field} must be an http(s) URL"
        )));
    }
    Ok(())
}

fn ensure_redirect_flow(flow: &PaymentFlow) -> Result<(), PaymentError> {
    match flow {
        PaymentFlow::Redirect { .. } | PaymentFlow::ClientSdk { .. } => Ok(()),
        _ => Err(PaymentError::validation(
            "alipay page/wap payments require a redirect (or client-SDK) flow",
        )),
    }
}

fn ensure_app_flow(flow: &PaymentFlow) -> Result<(), PaymentError> {
    match flow {
        PaymentFlow::App { .. } | PaymentFlow::ClientSdk { .. } => Ok(()),
        _ => Err(PaymentError::validation(
            "alipay app payments require an app (or client-SDK) flow",
        )),
    }
}

fn ensure_qr_flow(flow: &PaymentFlow) -> Result<(), PaymentError> {
    match flow {
        PaymentFlow::QrCode => Ok(()),
        _ => Err(PaymentError::validation(
            "alipay QR payments require the qr_code flow",
        )),
    }
}

fn validate_order_id(order_id: &str) -> Result<&str, PaymentError> {
    if order_id.trim().is_empty() {
        return Err(PaymentError::validation("order_id must not be empty"));
    }
    Ok(order_id)
}

/// Split the adapter's refund lookup id into the pair Alipay requires.
///
/// `alipay.trade.fastpay.refund.query` identifies a refund by the merchant
/// order number **and** the refund request number, so `get_refund` takes
/// `out_trade_no:out_request_no`.
fn split_refund_lookup_id(refund_id: &str) -> Result<(&str, &str), PaymentError> {
    let (order_id, out_request_no) = refund_id.split_once(':').ok_or_else(|| {
        PaymentError::validation(
            "alipay refund lookup requires `out_trade_no:out_request_no` \
             (alipay.trade.fastpay.refund.query needs both values)",
        )
    })?;
    if order_id.trim().is_empty() || out_request_no.trim().is_empty() {
        return Err(PaymentError::validation(
            "alipay refund lookup id must be `out_trade_no:out_request_no` with both parts non-empty",
        ));
    }
    Ok((order_id, out_request_no))
}

fn intent_from_query(
    reply: &GatewayReply,
    account: &AlipayAccount,
    requested_order_id: &str,
) -> Result<PaymentIntent, PaymentError> {
    let status_raw = string_field(&reply.node, "trade_status").ok_or_else(|| {
        PaymentError::Serialization("alipay trade query response has no trade_status".to_string())
    })?;
    let amount = string_field(&reply.node, "total_amount")
        .and_then(|value| money_cny(&value))
        .ok_or_else(|| {
            PaymentError::Serialization(
                "alipay trade query response has an invalid total_amount".to_string(),
            )
        })?;
    let order_id =
        string_field(&reply.node, "out_trade_no").unwrap_or_else(|| requested_order_id.to_string());
    let mut intent = PaymentIntent::new(
        PROVIDER,
        account.id.clone(),
        order_id,
        amount,
        map_trade_status(&status_raw),
        status_raw,
    );
    intent.provider_payment_id = string_field(&reply.node, "trade_no");
    intent.amount_captured =
        string_field(&reply.node, "receipt_amount").and_then(|value| money_cny(&value));
    intent.request_id = reply.request_id.clone();
    Ok(intent)
}

/// Register Alipay on a [`HostBuilder`].
pub trait AlipayHostBuilderExt: Sized {
    /// Register the Alipay adapter with runtime configuration.
    ///
    /// Binds the `"Alipay"` configuration section through
    /// `add_options`, so appsettings/file reloads are picked up by the
    /// runtime cache (default 60s) or immediately after
    /// `Payments::invalidate`.
    fn add_alipay(self) -> Self;

    /// Register the adapter with a prebuilt static configuration.
    fn add_alipay_with(self, options: AlipayOptions) -> Self;
}

impl AlipayHostBuilderExt for HostBuilder {
    fn add_alipay(self) -> Self {
        let builder = self.add_options::<AlipayOptions>("Alipay");
        builder.register(|services| {
            services
                .add_payment_provider_factory_lazy(|resolver| {
                    let monitor = resolver
                        .resolve_arc::<dyn webx_core::options_monitor::IOptionsMonitor<AlipayOptions>>(
                        )
                        .expect("add_alipay() registers the Alipay options monitor");
                    Arc::new(AlipayProviderFactory::new(monitor))
                })
                .add_payment_provider_builder(Arc::new(AlipayProviderBuilder))
        })
    }

    fn add_alipay_with(self, options: AlipayOptions) -> Self {
        let provider = Arc::new(AlipayProvider::new(options));
        let health = Arc::clone(&provider);
        self.register(move |services| {
            services
                .add_payment_provider(provider)
                .add_payment_provider_builder(Arc::new(AlipayProviderBuilder))
        })
        .add_health_check(HealthCheckSpec::sync("payments.alipay", move || {
            health.health()
        }))
    }
}

/// Runtime provider factory for Alipay (reads the options monitor).
pub struct AlipayProviderFactory {
    monitor: Arc<dyn webx_core::options_monitor::IOptionsMonitor<AlipayOptions>>,
}

impl AlipayProviderFactory {
    /// Build the factory over an options monitor.
    pub fn new(
        monitor: Arc<dyn webx_core::options_monitor::IOptionsMonitor<AlipayOptions>>,
    ) -> Self {
        Self { monitor }
    }
}

#[async_trait]
impl IPaymentProviderFactory for AlipayProviderFactory {
    fn provider(&self) -> ProviderId {
        PROVIDER
    }

    async fn resolve(
        &self,
        _scope: &PaymentScope,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        let options = self.monitor.current();
        let provider = AlipayProvider::new((*options).clone());
        provider.validate()?;
        Ok(Arc::new(provider))
    }
}

/// Builder used by custom `IPaymentServiceFactory` implementations.
#[derive(Debug, Default, Clone, Copy)]
pub struct AlipayProviderBuilder;

impl IPaymentProviderBuilder for AlipayProviderBuilder {
    fn provider(&self) -> ProviderId {
        PROVIDER
    }

    fn build(
        &self,
        config: &PaymentProviderConfig,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        let options: AlipayOptions =
            serde_json::from_value(config.settings.clone()).map_err(|error| {
                PaymentError::config(
                    PROVIDER,
                    format!("invalid Alipay provider settings: {error}"),
                )
            })?;
        let provider = AlipayProvider::new(options);
        provider.validate()?;
        Ok(Arc::new(provider))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use webx_pay::Money;

    use crate::test_support;
    use crate::test_support::scope;

    fn page_request(method: PaymentMethod, flow: PaymentFlow) -> CreatePayment {
        CreatePayment::new("ORDER-1", Money::cny(9900), method, flow)
            .with_description("Pro plan")
            .with_notify_url("https://example.com/notify")
    }

    fn multi_account_provider() -> AlipayProvider {
        AlipayProvider::new(test_support::options().with_account(
            "eu",
            "2021000000000002",
            test_support::merchant_private_key_pem(),
            crate::AlipayKeyMode::public_key(test_support::alipay_public_key_pem()),
        ))
    }

    fn redirect_flow() -> PaymentFlow {
        PaymentFlow::Redirect {
            success_url: "https://example.com/return".to_string(),
            cancel_url: Some("https://example.com/cancel".to_string()),
        }
    }

    fn query_pairs(url: &str) -> Vec<(String, String)> {
        let parsed = url::Url::parse(url).expect("valid redirect url");
        parsed
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect()
    }

    fn assert_signed_form(params: &[(String, String)]) {
        let signature = params
            .iter()
            .find(|(key, _)| key == "sign")
            .map(|(_, value)| value.clone())
            .expect("sign parameter");
        let content = crate::sign::request_sign_content(params);
        crate::sign::rsa2_verify(&content, &signature, test_support::merchant_public_key())
            .expect("redirect signature verifies");
    }

    fn biz_content(params: &[(String, String)]) -> Value {
        let value = params
            .iter()
            .find(|(key, _)| key == "biz_content")
            .map(|(_, value)| value.clone())
            .expect("biz_content parameter");
        serde_json::from_str(&value).expect("valid biz_content JSON")
    }

    #[test]
    fn capabilities_declare_the_supported_operations() {
        let capabilities = test_support::provider().capabilities();
        assert!(capabilities.create);
        assert!(capabilities.query);
        assert!(!capabilities.capture);
        assert!(capabilities.cancel);
        assert!(capabilities.refund);
        assert!(capabilities.partial_refund);
        assert!(capabilities.hosted_checkout);
        assert!(capabilities.qr);
        assert!(capabilities.app);
        assert_eq!(capabilities.webhook, WebhookCapability::Rsa2);
        assert!(capabilities.supports_currency(&Currency::CNY));
        assert!(!capabilities.supports_currency(&Currency::USD));
    }

    #[test]
    fn provider_validate_parses_keys_without_network() {
        assert!(test_support::provider().validate().is_ok());
        assert_eq!(test_support::provider().health().status, "pass");
    }

    #[test]
    fn payment_options_cover_every_account_and_channel() {
        let provider = multi_account_provider();
        let options = provider.payment_options();
        assert_eq!(options.len(), provider.accounts().len() * 4);

        let methods_for = |account: &str| -> Vec<&str> {
            options
                .iter()
                .filter(|option| option.account.as_str() == account)
                .map(|option| option.method.as_str())
                .collect()
        };
        let expected = ["alipay_page", "alipay_wap", "alipay_app", "alipay_qr"];
        assert_eq!(methods_for("default"), expected);
        assert_eq!(methods_for("eu"), expected);

        for option in &options {
            assert_eq!(option.provider, PROVIDER);
            assert_eq!(option.flows.len(), 1);
            let expected_app_id = if option.account.as_str() == "default" {
                test_support::APP_ID
            } else {
                "2021000000000002"
            };
            let app_id = option
                .client_config
                .as_ref()
                .and_then(|config| config.get("app_id"))
                .and_then(Value::as_str)
                .expect("catalog option carries the public app_id");
            assert_eq!(app_id, expected_app_id);
        }
    }

    #[test]
    fn payment_option_flows_match_the_channel() {
        let options = test_support::provider().payment_options();
        assert!(!options.is_empty());
        for option in options {
            let expected = match &option.method {
                PaymentMethod::AlipayPage | PaymentMethod::AlipayWap => PaymentFlowKind::Redirect,
                PaymentMethod::AlipayApp => PaymentFlowKind::App,
                PaymentMethod::AlipayQr => PaymentFlowKind::QrCode,
                method => panic!("unexpected catalog method `{method}`"),
            };
            assert_eq!(option.flows, vec![expected]);
        }
    }

    #[test]
    fn payment_option_client_config_exposes_only_the_public_app_id() {
        let options = test_support::provider().payment_options();
        assert!(!options.is_empty());
        for option in options {
            assert_eq!(
                option.client_config,
                Some(serde_json::json!({ "app_id": test_support::APP_ID }))
            );
        }
    }

    #[test]
    fn payment_option_serialization_is_public_only() {
        let options = multi_account_provider().payment_options();
        let json = serde_json::to_string(&options).expect("catalog serializes");
        assert!(json.contains(test_support::APP_ID));
        assert!(!json.contains("PRIVATE KEY"));
        assert!(!json
            .contains(test_support::pem_body(test_support::merchant_private_key_pem()).as_str()));
        assert!(
            !json.contains(test_support::pem_body(test_support::alipay_public_key_pem()).as_str())
        );
        assert!(!json.contains("private_key"));
        assert!(!json.contains("app_cert_sn"));
        assert!(!json.contains("alipay_root_cert_sn"));
        let decoded: Vec<PaymentOption> =
            serde_json::from_str(&json).expect("catalog deserializes");
        assert_eq!(decoded, options);
    }

    #[test]
    fn certificate_mode_catalog_is_public_only() {
        let options = AlipayOptions {
            accounts: vec![test_support::certificate_account_options()],
            ..AlipayOptions::default()
        };
        let payment_options = AlipayProvider::new(options).payment_options();
        assert!(!payment_options.is_empty());
        let json = serde_json::to_string(&payment_options).expect("catalog serializes");
        assert!(json.contains(test_support::APP_ID));
        assert!(!json.contains("CERTIFICATE"));
        assert!(!json.contains(test_support::certificates().app_cert_pem.as_str()));
        assert!(!json.contains(test_support::certificates().alipay_public_cert_pem.as_str()));
    }

    #[tokio::test]
    async fn payment_provider_contract_holds() {
        webx_pay::testing::payment_provider_contract(test_support::provider()).await;
    }

    #[tokio::test]
    async fn page_payment_returns_a_signed_redirect_url() {
        let intent = test_support::provider()
            .create_payment(
                &scope(),
                page_request(PaymentMethod::AlipayPage, redirect_flow()),
            )
            .await
            .expect("page payment");
        assert_eq!(intent.status, PaymentStatus::RequiresAction);
        assert_eq!(intent.order_id, "ORDER-1");
        assert_eq!(intent.amount, Money::cny(9900));
        let Some(ClientAction::Redirect { url }) = intent.client_action else {
            panic!("expected redirect client action");
        };
        assert!(
            url.starts_with("https://openapi.alipay.com/gateway.do?"),
            "{url}"
        );
        let params = query_pairs(&url);
        assert_signed_form(&params);
        let fields: Vec<&str> = params.iter().map(|(key, _)| key.as_str()).collect();
        assert!(fields.contains(&"sign"));
        assert!(fields.contains(&"notify_url"));
        assert!(fields.contains(&"return_url"));
        assert_eq!(
            params
                .iter()
                .find(|(key, _)| key == "method")
                .map(|(_, value)| value.as_str()),
            Some(transport::METHOD_PAGE_PAY)
        );
        let biz = biz_content(&params);
        assert_eq!(biz["out_trade_no"], "ORDER-1");
        assert_eq!(biz["total_amount"], "99.00");
        assert_eq!(biz["subject"], "Pro plan");
        assert_eq!(biz["product_code"], PRODUCT_PAGE_PAY);
    }

    #[tokio::test]
    async fn wap_payment_maps_cancel_url_to_quit_url() {
        let intent = test_support::provider()
            .create_payment(
                &scope(),
                page_request(PaymentMethod::AlipayWap, redirect_flow()),
            )
            .await
            .expect("wap payment");
        let Some(ClientAction::Redirect { url }) = intent.client_action else {
            panic!("expected redirect client action");
        };
        let params = query_pairs(&url);
        assert_signed_form(&params);
        let biz = biz_content(&params);
        assert_eq!(biz["product_code"], PRODUCT_WAP_PAY);
        assert_eq!(biz["quit_url"], "https://example.com/cancel");
    }

    #[tokio::test]
    async fn app_payment_returns_an_order_string() {
        let request = page_request(
            PaymentMethod::AlipayApp,
            PaymentFlow::App { return_url: None },
        );
        let intent = test_support::provider()
            .create_payment(&scope(), request)
            .await
            .expect("app payment");
        let Some(ClientAction::AppOrder { provider, payload }) = intent.client_action else {
            panic!("expected app order client action");
        };
        assert_eq!(provider, PROVIDER);
        let order_str = payload["order_str"].as_str().expect("order_str");
        let params: Vec<(String, String)> = url::form_urlencoded::parse(order_str.as_bytes())
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        assert_signed_form(&params);
        let fields: Vec<&str> = params.iter().map(|(key, _)| key.as_str()).collect();
        assert!(fields.contains(&"sign"));
        assert!(fields.contains(&"sign_type"));
        let biz = biz_content(&params);
        assert_eq!(biz["product_code"], PRODUCT_APP_PAY);
    }

    #[tokio::test]
    async fn create_rejects_wrong_flow_currency_capture_and_unsupported_methods() {
        let provider = test_support::provider();
        let error = provider
            .create_payment(
                &scope(),
                page_request(PaymentMethod::AlipayPage, PaymentFlow::QrCode),
            )
            .await
            .unwrap_err();
        assert_eq!(error.http_status(), 400);

        let error = provider
            .create_payment(
                &scope(),
                page_request(
                    PaymentMethod::AlipayApp,
                    PaymentFlow::Redirect {
                        success_url: "https://example.com/ok".to_string(),
                        cancel_url: None,
                    },
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(error.http_status(), 400);

        let mut request = page_request(PaymentMethod::AlipayQr, PaymentFlow::QrCode);
        request.amount = Money::usd(100);
        let error = provider
            .create_payment(&scope(), request)
            .await
            .unwrap_err();
        assert_eq!(error.http_status(), 400);

        let mut request = page_request(PaymentMethod::AlipayPage, redirect_flow());
        request.capture = CaptureMode::Manual;
        assert!(matches!(
            provider
                .create_payment(&scope(), request)
                .await
                .unwrap_err(),
            PaymentError::Unsupported { .. }
        ));

        let error = provider
            .create_payment(
                &scope(),
                page_request(
                    PaymentMethod::Card,
                    PaymentFlow::ClientSdk { return_url: None },
                ),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, PaymentError::Unsupported { .. }));
    }

    #[tokio::test]
    async fn create_accepts_a_client_sdk_return_url_for_page_payments() {
        let request = page_request(
            PaymentMethod::AlipayPage,
            PaymentFlow::ClientSdk {
                return_url: Some("https://example.com/return".to_string()),
            },
        );
        let intent = test_support::provider()
            .create_payment(&scope(), request)
            .await
            .expect("page payment");
        let Some(ClientAction::Redirect { url }) = intent.client_action else {
            panic!("expected redirect client action");
        };
        let params = query_pairs(&url);
        assert!(params
            .iter()
            .any(|(key, value)| key == "return_url" && value == "https://example.com/return"));
    }

    #[tokio::test]
    async fn unknown_account_is_a_validation_error() {
        let scope = scope().account("missing");
        let error = test_support::provider()
            .create_payment(
                &scope,
                page_request(PaymentMethod::AlipayPage, redirect_flow()),
            )
            .await
            .unwrap_err();
        assert_eq!(error.http_status(), 400);
        assert!(error.to_string().contains("unknown alipay account"));
    }

    #[tokio::test]
    async fn get_payment_requires_an_order_id() {
        let error = test_support::provider()
            .get_payment(&scope(), "  ")
            .await
            .unwrap_err();
        assert_eq!(error.http_status(), 400);
    }

    #[tokio::test]
    async fn get_refund_requires_the_pair_alipay_documents() {
        let error = test_support::provider()
            .get_refund(&scope(), "REFUND-1")
            .await
            .unwrap_err();
        assert_eq!(error.http_status(), 400);
        assert!(error.to_string().contains("out_trade_no:out_request_no"));
    }

    #[test]
    fn certificate_mode_provider_validates() {
        let options = AlipayOptions {
            accounts: vec![test_support::certificate_account_options()],
            ..AlipayOptions::default()
        };
        let provider = AlipayProvider::new(options);
        assert!(provider.validate().is_ok());
        assert_eq!(
            provider.accounts()[0].provider,
            PROVIDER,
            "account views are owned by alipay"
        );
    }

    #[test]
    fn invalid_configuration_fails_validation() {
        let provider = AlipayProvider::new(AlipayOptions::empty());
        let error = provider.validate().unwrap_err();
        assert_eq!(error.http_status(), 500);
        assert!(provider.accounts().is_empty());
        assert!(provider.payment_options().is_empty());
        assert_eq!(provider.health().status, "fail");
    }

    #[test]
    fn webhook_verifier_belongs_to_alipay() {
        let provider = test_support::provider();
        assert_eq!(provider.webhook_verifier().provider(), PROVIDER);
    }

    #[test]
    fn order_ids_and_refund_lookup_ids_are_validated() {
        assert!(validate_order_id("ORDER-1").is_ok());
        assert!(validate_order_id(" ").is_err());
        assert_eq!(
            split_refund_lookup_id("ORDER-1:REFUND-1").unwrap(),
            ("ORDER-1", "REFUND-1")
        );
        assert!(split_refund_lookup_id("REFUND-1").is_err());
        assert!(split_refund_lookup_id(":REFUND-1").is_err());
        assert!(split_refund_lookup_id("ORDER-1:").is_err());
    }

    #[test]
    fn redirect_url_is_built_from_the_configured_gateway() {
        let options = test_support::options().with_gateway("https://proxy.example.com/gateway.do");
        let provider = AlipayProvider::new(options);
        assert_eq!(
            provider.gateway().unwrap(),
            "https://proxy.example.com/gateway.do"
        );
    }

    #[tokio::test]
    async fn provider_with_explicit_client_is_used_for_gateway_calls() {
        // The explicit client build is what tests and shared-pool deployments
        // use; no request is sent here, but validation must accept it.
        struct UnusedClient;
        #[async_trait]
        impl IHttpClient for UnusedClient {
            fn name(&self) -> &str {
                "unused"
            }

            async fn execute(
                &self,
                _request: OutboundRequest,
            ) -> Result<webx_client::OutboundResponse, webx_client::HttpClientError> {
                Err(webx_client::HttpClientError::InvalidRequest(
                    "not used in this test".to_string(),
                ))
            }
        }
        let provider = AlipayProvider::with_client(test_support::options(), Arc::new(UnusedClient));
        assert!(provider.validate().is_ok());
    }
}
