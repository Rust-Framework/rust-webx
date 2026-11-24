//! WeChat Pay APIv3 provider, request bodies, response mapping and
//! registration.
//!
//! Wire traffic (all JSON, merchant-signed):
//!
//! | operation | request |
//! | --- | --- |
//! | create Native | `POST /v3/pay/transactions/native` → `code_url` |
//! | create JSAPI | `POST /v3/pay/transactions/jsapi` → `prepay_id` |
//! | create App | `POST /v3/pay/transactions/app` → `prepay_id` |
//! | create H5 | `POST /v3/pay/transactions/h5` → `h5_url` |
//! | query | `GET /v3/pay/transactions/out-trade-no/{out_trade_no}?mchid=` |
//! | close | `POST /v3/pay/transactions/out-trade-no/{out_trade_no}/close` (204) |
//! | refund | `POST /v3/refund/domestic/refunds` |
//! | refund query | `GET /v3/refund/domestic/refunds/{out_refund_no}` |
//!
//! Amounts are integer fen (`amount.total` / `amount.refund`); the currency is
//! fixed to CNY. `out_trade_no` and `out_refund_no` are the idempotency anchors:
//! retrying a failed request must reuse the same merchant ids.

use std::fmt;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use rsa::{PrivateKey, PublicKey};
use rust_dix::IServiceResolverExt;
use serde_json::{json, Value};
use webx_client::IHttpClient;
use webx_host::health::HealthCheckSpec;
use webx_host::health::HealthStatus;
use webx_host::server::HostBuilder;
use webx_pay::{
    AccountId, AccountSet, CaptureMode, ClientAction, CreatePayment, Currency, CurrencyScope,
    IPaymentProvider, IPaymentProviderBuilder, IPaymentProviderFactory, IPaymentWebhookVerifier,
    PaymentAccount, PaymentError, PaymentFlowKind, PaymentIntent, PaymentMethod, PaymentOption,
    PaymentProviderConfig, PaymentScope, PaymentStatus, PaymentsServiceCollectionExt,
    ProviderCapabilities, ProviderId, Refund, RefundCommand, WebhookCapability,
};

use crate::cert::{normalize_serial, CertificateCache};
use crate::mapping::{
    amount_field, map_refund_status, map_trade_state, money_from_fen, string_field,
};
use crate::options::{WechatPayAccountOptions, WechatPayKeyMode, WechatPayOptions};
use crate::sign::{
    app_client_signature_message, jsapi_client_signature_message, new_nonce, parse_private_key_pem,
    parse_public_key_pem, sign_message_base64,
};
use crate::time::{format_rfc3339, parse_rfc3339, unix_now};
use crate::transport::WechatPayTransport;
use crate::webhook::WechatPayWebhookVerifier;
use crate::PROVIDER;

const CREATE_NATIVE_PATH: &str = "/v3/pay/transactions/native";
const CREATE_JSAPI_PATH: &str = "/v3/pay/transactions/jsapi";
const CREATE_APP_PATH: &str = "/v3/pay/transactions/app";
const CREATE_H5_PATH: &str = "/v3/pay/transactions/h5";
const REFUND_PATH: &str = "/v3/refund/domestic/refunds";

/// Why WeChat Pay requires OpenSDK parameters as strings: timestamps are
/// seconds since the Unix epoch in both JSAPI and App payloads.
const JSAPI_SIGN_TYPE: &str = "RSA";
/// App PayReq package value.
const APP_PACKAGE_VALUE: &str = "Sign=WXPay";

/// Resolved credentials and verification mode for one merchant account.
#[derive(Clone)]
pub(crate) struct WechatPayAccount {
    pub(crate) id: AccountId,
    pub(crate) mch_id: String,
    pub(crate) app_id: String,
    pub(crate) serial_no: String,
    pub(crate) signing_key: Arc<PrivateKey>,
    pub(crate) api_v3_key: [u8; 32],
    pub(crate) notify_url: String,
    pub(crate) live: bool,
    pub(crate) key_mode: AccountKeyMode,
}

impl WechatPayAccount {
    pub(crate) fn from_options(options: &WechatPayAccountOptions) -> Result<Self, String> {
        let id = AccountId::new(options.id.clone())
            .map_err(|error| format!("invalid wechatpay account id: {error}"))?;
        let signing_key = Arc::new(parse_private_key_pem(&options.private_key_pem)?);
        let api_v3_key: [u8; 32] = options
            .api_v3_key
            .as_bytes()
            .try_into()
            .map_err(|_| "APIv3 key must be exactly 32 bytes".to_string())?;
        let key_mode = match &options.key_mode {
            WechatPayKeyMode::PlatformCertificates => {
                AccountKeyMode::PlatformCertificates(Arc::new(CertificateCache::new()))
            }
            WechatPayKeyMode::PublicKey {
                public_key_id,
                public_key_pem,
            } => AccountKeyMode::PublicKey {
                public_key_id: public_key_id.trim().to_string(),
                public_key: Arc::new(parse_public_key_pem(public_key_pem)?),
            },
        };
        Ok(Self {
            id,
            mch_id: options.mch_id.trim().to_string(),
            app_id: options.app_id.trim().to_string(),
            serial_no: normalize_serial(&options.serial_no),
            signing_key,
            api_v3_key,
            notify_url: options.notify_url.trim().to_string(),
            live: options.live.unwrap_or(true),
            key_mode,
        })
    }

    /// Default callback URL resolved for one operation (request override wins).
    fn resolve_notify_url(&self, requested: Option<&str>) -> Result<String, PaymentError> {
        let resolved = requested
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_string)
            .or_else(|| {
                (!self.notify_url.is_empty()).then(|| self.notify_url.clone())
            })
            .ok_or_else(|| {
                PaymentError::config(
                    PROVIDER,
                    format!(
                        "wechatpay account `{}` has no notify_url and the request did not provide one",
                        self.id
                    ),
                )
            })?;
        if !resolved.starts_with("https://") {
            return Err(PaymentError::validation(
                "wechatpay notify_url must be an https URL",
            ));
        }
        Ok(resolved)
    }
}

impl fmt::Debug for WechatPayAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WechatPayAccount")
            .field("id", &self.id)
            .field("mch_id", &self.mch_id)
            .field("app_id", &self.app_id)
            .field("serial_no", &self.serial_no)
            .field("signing_key", &"<redacted>")
            .field("api_v3_key", &"<redacted>")
            .field("notify_url", &self.notify_url)
            .field("live", &self.live)
            .field("key_mode", &self.key_mode)
            .finish()
    }
}

/// Response-verification mode for one account.
#[derive(Clone)]
pub(crate) enum AccountKeyMode {
    /// Download/rotate platform certificates from `GET /v3/certificates`.
    PlatformCertificates(Arc<CertificateCache>),
    /// Verify with the WeChat Pay public key (`PUB_KEY_ID_...` mode).
    PublicKey {
        public_key_id: String,
        public_key: Arc<PublicKey>,
    },
}

impl fmt::Debug for AccountKeyMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlatformCertificates(cache) => f
                .debug_struct("PlatformCertificates")
                .field("cached_certificates", &cache.len())
                .finish(),
            Self::PublicKey { public_key_id, .. } => f
                .debug_struct("PublicKey")
                .field("public_key_id", public_key_id)
                .finish(),
        }
    }
}

/// Request channel selected by [`PaymentMethod`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    Native,
    Jsapi,
    App,
    H5,
}

impl Channel {
    fn from_method(method: &PaymentMethod) -> Result<Self, PaymentError> {
        match method {
            PaymentMethod::WechatNative => Ok(Self::Native),
            PaymentMethod::WechatJsapi => Ok(Self::Jsapi),
            PaymentMethod::WechatApp => Ok(Self::App),
            PaymentMethod::WechatH5 => Ok(Self::H5),
            _ => Err(PaymentError::unsupported(PROVIDER, "payment_method")),
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Native => CREATE_NATIVE_PATH,
            Self::Jsapi => CREATE_JSAPI_PATH,
            Self::App => CREATE_APP_PATH,
            Self::H5 => CREATE_H5_PATH,
        }
    }
}

/// WeChat Pay APIv3 adapter.
///
/// One instance serves one [`ProviderId::WechatPay`] and any number of merchant
/// accounts. Configuration is validated synchronously by [`WechatPayProvider::validate`];
/// no network call happens before the first payment operation.
///
/// The frontend catalog (`IPaymentProvider::payment_options`) exposes the four
/// APIv3 channels per account — Native/QR, JSAPI/client SDK, App and
/// H5/redirect — with only the public `app_id` in `client_config`; secret
/// material (merchant private key, APIv3 key, certificate serial) never enters
/// the catalog.
pub struct WechatPayProvider {
    accounts: Result<AccountSet<WechatPayAccount>, String>,
    transport: Arc<WechatPayTransport>,
    verifier: WechatPayWebhookVerifier,
    validated: OnceLock<Result<(), String>>,
}

impl WechatPayProvider {
    /// Build a provider from options (validated before first use by the
    /// runtime factory, or at registration time by `add_wechat_pay_with`).
    pub fn new(options: WechatPayOptions) -> Self {
        let transport = Arc::new(WechatPayTransport::new(options.http_options()));
        Self::with_transport(options, transport)
    }

    /// Build a provider with an explicit HTTP client (tests, shared pool).
    pub fn with_client(options: WechatPayOptions, client: Arc<dyn IHttpClient>) -> Self {
        let transport = Arc::new(WechatPayTransport::with_client(
            options.http_options(),
            client,
        ));
        Self::with_transport(options, transport)
    }

    fn with_transport(options: WechatPayOptions, transport: Arc<WechatPayTransport>) -> Self {
        let accounts = options.build_accounts();
        let verifier = WechatPayWebhookVerifier::new(accounts.clone(), Arc::clone(&transport));
        Self {
            accounts,
            transport,
            verifier,
            validated: OnceLock::new(),
        }
    }

    /// Synchronous health probe: configuration parses and the HTTP client
    /// builds. Network reachability is a readiness concern, not a liveness one.
    pub fn health(&self) -> HealthStatus {
        match self.validate() {
            Ok(()) => HealthStatus::pass(),
            Err(error) => HealthStatus::fail(error.to_string()),
        }
    }

    /// Download and cache the WeChat Pay platform certificates for every
    /// account that uses
    /// [`WechatPayKeyMode::PlatformCertificates`](crate::WechatPayKeyMode).
    ///
    /// Official guidance is to refresh on a schedule well below 12 hours (the
    /// certificates are rotated periodically). An unknown `Wechatpay-Serial` is
    /// also refreshed lazily and rate-limited by the adapter; calling this
    /// method is the operator-driven path (for example from a background
    /// hosted service). Public-key-mode accounts are a no-op.
    ///
    /// The certificate download response is the bootstrap trust anchor and is
    /// not signature-verified (official exemption; see the crate docs).
    pub async fn refresh_platform_certificates(&self) -> Result<(), PaymentError> {
        self.validate()?;
        let accounts = self.account_set()?;
        for id in accounts.ids() {
            let account = accounts.resolve(Some(id))?;
            self.transport.refresh_certificates(account).await?;
        }
        Ok(())
    }

    fn account_set(&self) -> Result<&AccountSet<WechatPayAccount>, PaymentError> {
        self.accounts
            .as_ref()
            .map_err(|error| PaymentError::config(PROVIDER, error.clone()))
    }

    fn account_for(
        &self,
        requested: Option<&AccountId>,
    ) -> Result<&Arc<WechatPayAccount>, PaymentError> {
        self.account_set()?.resolve(requested)
    }

    async fn fetch_payment(
        &self,
        account: &WechatPayAccount,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        let path = format!(
            "/v3/pay/transactions/out-trade-no/{order_id}?mchid={}",
            account.mch_id
        );
        let response = self.transport.get_json(account, &path).await?;
        let mut intent = parse_trade(&response.value, account, order_id)?;
        intent.request_id = response.request_id;
        Ok(intent)
    }
}

#[async_trait]
impl IPaymentProvider for WechatPayProvider {
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
            webhook: WebhookCapability::Rsa2Aes256Gcm,
            currencies: CurrencyScope::Only(&["CNY"]),
            statuses: &[
                PaymentStatus::RequiresAction,
                PaymentStatus::Processing,
                PaymentStatus::Succeeded,
                PaymentStatus::Refunded,
                PaymentStatus::Canceled,
                PaymentStatus::Failed,
            ],
        }
    }

    fn payment_options(&self) -> Vec<PaymentOption> {
        let Ok(accounts) = self.account_set() else {
            return Vec::new();
        };
        let mut options = Vec::with_capacity(accounts.len() * 4);
        for (account_id, account) in accounts.iter() {
            // Only public identifiers may enter the catalog: `app_id` is public
            // (backend and frontend bind to the same app); the merchant private
            // key, the APIv3 key and the certificate serial stay private.
            let label = accounts
                .accounts()
                .iter()
                .find(|info| &info.id == account_id)
                .and_then(|info| info.label.clone());
            for (method, flow) in [
                (PaymentMethod::WechatNative, PaymentFlowKind::QrCode),
                (PaymentMethod::WechatJsapi, PaymentFlowKind::ClientSdk),
                (PaymentMethod::WechatApp, PaymentFlowKind::App),
                (PaymentMethod::WechatH5, PaymentFlowKind::Redirect),
            ] {
                let mut option = PaymentOption::new(PROVIDER, account_id.clone(), method, [flow])
                    .with_client_config(json!({ "app_id": account.app_id }));
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
            self.transport.client()?;
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
        let channel = validate_create_request(&request)?;
        let account = self.account_for(scope.account_ref())?;
        let notify_url = account.resolve_notify_url(request.notify_url.as_deref())?;
        let body = build_create_body(account, &request, channel, &notify_url)?;
        let response = self
            .transport
            .post_json(account, channel.path(), &body)
            .await?;
        let client_action = client_action_for(channel, account, &response.value)?;
        let mut intent = PaymentIntent::new(
            PROVIDER,
            account.id.clone(),
            &request.order_id,
            request.amount,
            PaymentStatus::RequiresAction,
            "NOTPAY",
        );
        intent.client_action = Some(client_action);
        intent.created_at = unix_now();
        intent.expires_at = request.expires_at;
        intent.request_id = response.request_id;
        Ok(intent)
    }

    async fn get_payment(
        &self,
        scope: &PaymentScope,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        let order_id = validate_trade_no(order_id)?;
        let account = self.account_for(scope.account_ref())?;
        self.fetch_payment(account, order_id).await
    }

    async fn cancel_payment(
        &self,
        scope: &PaymentScope,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        let order_id = validate_trade_no(order_id)?;
        let account = self.account_for(scope.account_ref())?;
        let path = format!("/v3/pay/transactions/out-trade-no/{order_id}/close");
        let body = json!({ "mchid": account.mch_id });
        self.transport.post_json(account, &path, &body).await?;
        // Closing returns `204 No Content`; read the order back so the caller
        // gets the authoritative CLOSED state and amount.
        self.fetch_payment(account, order_id).await
    }

    async fn refund(
        &self,
        scope: &PaymentScope,
        request: RefundCommand,
    ) -> Result<Refund, PaymentError> {
        request.validate()?;
        let refund_no = validate_refund_no(&request.refund_id)?;
        if request.reason.len() > 80 {
            return Err(PaymentError::validation(
                "wechatpay refund reason must be at most 80 bytes",
            ));
        }
        let order_id = validate_trade_no(&request.order_id)?;
        let account = self.account_for(scope.account_ref())?;
        // WeChat requires the original order amount in `amount.total`, which is
        // only available from the order; query it first (also fails fast for an
        // unknown/unpaid order).
        let order = self.fetch_payment(account, order_id).await?;
        let total = order.amount.minor;
        if total <= 0 {
            return Err(PaymentError::Serialization(
                "wechatpay order response has no positive amount.total".to_string(),
            ));
        }
        let refunded = match &request.amount {
            Some(amount) => {
                if amount.currency != Currency::CNY {
                    return Err(PaymentError::validation(format!(
                        "wechatpay supports CNY only, got {}",
                        amount.currency
                    )));
                }
                amount.minor
            }
            None => total,
        };
        if refunded <= 0 || refunded > total {
            return Err(PaymentError::validation(
                "wechatpay refund amount must be positive and must not exceed the order total",
            ));
        }
        let mut body = json!({
            "out_trade_no": order_id,
            "out_refund_no": refund_no,
            "amount": {
                "refund": refunded,
                "total": total,
                "currency": "CNY",
            },
        });
        if !request.reason.trim().is_empty() {
            body["reason"] = json!(request.reason.trim());
        }
        if let Some(notify_url) = request
            .notify_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
        {
            if !notify_url.starts_with("https://") {
                return Err(PaymentError::validation(
                    "wechatpay notify_url must be an https URL",
                ));
            }
            body["notify_url"] = json!(notify_url);
        }
        let response = self
            .transport
            .post_json(account, REFUND_PATH, &body)
            .await?;
        let mut refund = parse_refund(&response.value, account, Some(order_id), Some(refund_no))?;
        refund.request_id = response.request_id;
        Ok(refund)
    }

    async fn get_refund(
        &self,
        scope: &PaymentScope,
        refund_id: &str,
    ) -> Result<Refund, PaymentError> {
        let refund_no = validate_refund_no(refund_id)?;
        let account = self.account_for(scope.account_ref())?;
        let path = format!("/v3/refund/domestic/refunds/{refund_no}");
        let response = self.transport.get_json(account, &path).await?;
        let mut refund = parse_refund(&response.value, account, None, Some(refund_no))?;
        refund.request_id = response.request_id;
        Ok(refund)
    }

    fn webhook_verifier(&self) -> &dyn IPaymentWebhookVerifier {
        &self.verifier
    }
}

/// Channel/currency/capture checks that never need the merchant account.
fn validate_create_request(request: &CreatePayment) -> Result<Channel, PaymentError> {
    if request.amount.currency != Currency::CNY {
        return Err(PaymentError::validation(format!(
            "wechatpay supports CNY only, got {}",
            request.amount.currency
        )));
    }
    if request.capture == CaptureMode::Manual {
        return Err(PaymentError::unsupported(PROVIDER, "capture"));
    }
    let channel = Channel::from_method(&request.method)?;
    validate_trade_no(&request.order_id)?;
    Ok(channel)
}

/// Build the documented create-order body for `channel`.
fn build_create_body(
    account: &WechatPayAccount,
    request: &CreatePayment,
    channel: Channel,
    notify_url: &str,
) -> Result<Value, PaymentError> {
    let description = if request.description.trim().is_empty() {
        format!("Order {}", request.order_id)
    } else {
        request.description.trim().to_string()
    };
    if description.chars().count() > 127 {
        return Err(PaymentError::validation(
            "wechatpay description must be at most 127 characters",
        ));
    }
    let mut body = json!({
        "appid": account.app_id,
        "mchid": account.mch_id,
        "description": description,
        "out_trade_no": request.order_id,
        "notify_url": notify_url,
        "amount": {
            "total": request.amount.minor,
            "currency": "CNY",
        },
    });
    if let Some(expires_at) = request.expires_at {
        body["time_expire"] = json!(format_rfc3339(expires_at));
    }
    if let Some(attach) = request.metadata.get("attach") {
        if attach.chars().count() > 128 {
            return Err(PaymentError::validation(
                "wechatpay attach metadata must be at most 128 characters",
            ));
        }
        body["attach"] = json!(attach);
    }
    match channel {
        Channel::Native | Channel::App => {}
        Channel::Jsapi => {
            let openid = request
                .payer_ref
                .as_deref()
                .map(str::trim)
                .filter(|openid| !openid.is_empty())
                .ok_or_else(|| {
                    PaymentError::validation(
                        "wechatpay JSAPI payments require payer_ref (the payer openid)",
                    )
                })?;
            body["payer"] = json!({ "openid": openid });
        }
        Channel::H5 => {
            let client_ip = request.client_ip.ok_or_else(|| {
                PaymentError::validation("wechatpay H5 payments require client_ip")
            })?;
            body["scene_info"] = json!({
                "payer_client_ip": client_ip.to_string(),
                "h5_info": { "type": "Wap" },
            });
        }
    }
    if channel != Channel::H5 {
        if let Some(client_ip) = request.client_ip {
            body["scene_info"] = json!({ "payer_client_ip": client_ip.to_string() });
        }
    }
    Ok(body)
}

/// Map a create-order response to the client action for `channel`.
fn client_action_for(
    channel: Channel,
    account: &WechatPayAccount,
    value: &Value,
) -> Result<ClientAction, PaymentError> {
    match channel {
        Channel::Native => {
            let code_url = required_string(value, "code_url")?;
            Ok(ClientAction::QrCode {
                content: code_url.to_string(),
            })
        }
        Channel::H5 => {
            let h5_url = required_string(value, "h5_url")?;
            Ok(ClientAction::Redirect {
                url: h5_url.to_string(),
            })
        }
        Channel::Jsapi => {
            let prepay_id = required_string(value, "prepay_id")?;
            Ok(ClientAction::AppOrder {
                provider: PROVIDER,
                payload: jsapi_order_payload(account, prepay_id)?,
            })
        }
        Channel::App => {
            let prepay_id = required_string(value, "prepay_id")?;
            Ok(ClientAction::AppOrder {
                provider: PROVIDER,
                payload: app_order_payload(account, prepay_id)?,
            })
        }
    }
}

/// JSAPI / 小程序 client parameters (WeixinJSBridge `getBrandWCPayRequest`).
fn jsapi_order_payload(account: &WechatPayAccount, prepay_id: &str) -> Result<Value, PaymentError> {
    let timestamp = unix_now().to_string();
    let nonce = new_nonce();
    let package = format!("prepay_id={prepay_id}");
    let message = jsapi_client_signature_message(&account.app_id, &timestamp, &nonce, &package);
    let pay_sign = sign_message_base64(&account.signing_key, &message).map_err(|error| {
        PaymentError::Internal(format!("wechatpay JSAPI client signature failed: {error}"))
    })?;
    Ok(json!({
        "appId": account.app_id,
        "timeStamp": timestamp,
        "nonceStr": nonce,
        "package": package,
        "signType": JSAPI_SIGN_TYPE,
        "paySign": pay_sign,
    }))
}

/// App SDK client parameters (`PayReq`; iOS sends `packageValue` as `package`).
fn app_order_payload(account: &WechatPayAccount, prepay_id: &str) -> Result<Value, PaymentError> {
    let timestamp = unix_now().to_string();
    let nonce = new_nonce();
    let message = app_client_signature_message(&account.app_id, &timestamp, &nonce, prepay_id);
    let sign = sign_message_base64(&account.signing_key, &message).map_err(|error| {
        PaymentError::Internal(format!("wechatpay App client signature failed: {error}"))
    })?;
    Ok(json!({
        "appId": account.app_id,
        "partnerId": account.mch_id,
        "prepayId": prepay_id,
        "packageValue": APP_PACKAGE_VALUE,
        "nonceStr": nonce,
        "timeStamp": timestamp,
        "sign": sign,
    }))
}

/// Map a query-order response.
fn parse_trade(
    value: &Value,
    account: &WechatPayAccount,
    order_id: &str,
) -> Result<PaymentIntent, PaymentError> {
    let trade_state = required_string(value, "trade_state")?;
    let total = amount_field(value, "total").ok_or_else(|| {
        PaymentError::Serialization("wechatpay order response has no amount.total".to_string())
    })?;
    if total < 0 {
        return Err(PaymentError::Serialization(
            "wechatpay order response has a negative amount.total".to_string(),
        ));
    }
    let status = map_trade_state(trade_state);
    let mut intent = PaymentIntent::new(
        PROVIDER,
        account.id.clone(),
        order_id,
        money_from_fen(total),
        status,
        trade_state,
    );
    intent.provider_payment_id = string_field(value, "transaction_id").map(str::to_string);
    if status.is_paid() {
        intent.amount_captured = Some(money_from_fen(total));
    }
    // WeChat does not return the order creation time; `success_time` (when
    // paid) is the only timestamp on this response.
    intent.created_at = string_field(value, "success_time")
        .and_then(parse_rfc3339)
        .unwrap_or(0);
    intent.metadata = echo_metadata(value);
    Ok(intent)
}

/// Map a refund response (create or query).
fn parse_refund(
    value: &Value,
    account: &WechatPayAccount,
    fallback_order_id: Option<&str>,
    fallback_refund_id: Option<&str>,
) -> Result<Refund, PaymentError> {
    let provider_refund_id = required_string(value, "refund_id")?;
    let refund_status = required_string(value, "status")?;
    let order_id = string_field(value, "out_trade_no")
        .map(str::to_string)
        .or_else(|| fallback_order_id.map(str::to_string))
        .ok_or_else(|| {
            PaymentError::Serialization("wechatpay refund response has no out_trade_no".to_string())
        })?;
    let refund_id = string_field(value, "out_refund_no")
        .map(str::to_string)
        .or_else(|| fallback_refund_id.map(str::to_string))
        .ok_or_else(|| {
            PaymentError::Serialization(
                "wechatpay refund response has no out_refund_no".to_string(),
            )
        })?;
    if let Some(currency) = value
        .get("amount")
        .and_then(|amount| string_field(amount, "currency"))
    {
        if !currency.eq_ignore_ascii_case("CNY") {
            return Err(PaymentError::Serialization(format!(
                "unsupported WeChat Pay refund currency `{currency}`"
            )));
        }
    }
    let refunded = amount_field(value, "refund").ok_or_else(|| {
        PaymentError::Serialization("wechatpay refund response has no amount.refund".to_string())
    })?;
    if refunded < 0 {
        return Err(PaymentError::Serialization(
            "wechatpay refund response has a negative amount.refund".to_string(),
        ));
    }
    Ok(Refund {
        provider: PROVIDER,
        account: account.id.clone(),
        order_id,
        refund_id,
        provider_refund_id: Some(provider_refund_id.to_string()),
        status: map_refund_status(refund_status),
        provider_status: refund_status.to_string(),
        amount: money_from_fen(refunded),
        created_at: string_field(value, "create_time")
            .and_then(parse_rfc3339)
            .unwrap_or(0),
        succeeded_at: string_field(value, "success_time").and_then(parse_rfc3339),
        request_id: None,
    })
}

/// Echo the `attach` field (the only custom data WeChat Pay returns).
fn echo_metadata(value: &Value) -> std::collections::BTreeMap<String, String> {
    let mut metadata = std::collections::BTreeMap::new();
    if let Some(attach) = string_field(value, "attach").filter(|attach| !attach.is_empty()) {
        metadata.insert("attach".to_string(), attach.to_string());
    }
    metadata
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, PaymentError> {
    string_field(value, key)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| PaymentError::Serialization(format!("wechatpay response has no `{key}`")))
}

/// Merchant order id rules: 1-32 characters from `[A-Za-z0-9_-|*]`.
fn validate_trade_no(order_id: &str) -> Result<&str, PaymentError> {
    let order_id = order_id.trim();
    if order_id.is_empty() || order_id.len() > 32 {
        return Err(PaymentError::validation(
            "wechatpay out_trade_no must be 1-32 characters",
        ));
    }
    if !order_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'|' | b'*'))
    {
        return Err(PaymentError::validation(
            "wechatpay out_trade_no may only contain ASCII letters, digits, `_`, `-`, `|` and `*`",
        ));
    }
    Ok(order_id)
}

/// Merchant refund id rules: 1-64 bytes from `[A-Za-z0-9_-|*@]`.
fn validate_refund_no(refund_no: &str) -> Result<&str, PaymentError> {
    let refund_no = refund_no.trim();
    if refund_no.is_empty() || refund_no.len() > 64 {
        return Err(PaymentError::validation(
            "wechatpay out_refund_no must be 1-64 characters",
        ));
    }
    if !refund_no.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'|' | b'*' | b'@')
    }) {
        return Err(PaymentError::validation(
            "wechatpay out_refund_no may only contain ASCII letters, digits, `_`, `-`, `|`, `*` and `@`",
        ));
    }
    Ok(refund_no)
}

/// Register WeChat Pay on a [`HostBuilder`].
pub trait WechatPayHostBuilderExt: Sized {
    /// Register the adapter with runtime configuration.
    ///
    /// Binds the `"WeChatPay"` configuration section through
    /// `add_options_monitor`, so appsettings/file reloads are picked up by the
    /// runtime cache (default 60s) or immediately after `Payments::invalidate`.
    fn add_wechat_pay(self) -> Self;

    /// Register the adapter with a prebuilt static configuration.
    fn add_wechat_pay_with(self, options: WechatPayOptions) -> Self;
}

impl WechatPayHostBuilderExt for HostBuilder {
    fn add_wechat_pay(self) -> Self {
        self.add_options_monitor::<WechatPayOptions>("WeChatPay")
            .register(|services| {
                services
                    .add_payment_provider_factory_lazy(|resolver| {
                        let monitor = resolver
                            .resolve_arc::<dyn webx_core::options_monitor::IOptionsMonitor<WechatPayOptions>>()
                            .expect("add_wechat_pay() registers the WeChatPay options monitor");
                        Arc::new(WechatPayProviderFactory::new(monitor))
                    })
                    .add_payment_provider_builder(Arc::new(WechatPayProviderBuilder))
            })
    }

    fn add_wechat_pay_with(self, options: WechatPayOptions) -> Self {
        let provider = Arc::new(WechatPayProvider::new(options));
        let health = Arc::clone(&provider);
        self.register(move |services| {
            services
                .add_payment_provider(provider)
                .add_payment_provider_builder(Arc::new(WechatPayProviderBuilder))
        })
        .add_health_check(HealthCheckSpec::sync("payments.wechatpay", move || {
            health.health()
        }))
    }
}

/// Runtime provider factory for WeChat Pay (reads the options monitor).
pub struct WechatPayProviderFactory {
    monitor: Arc<dyn webx_core::options_monitor::IOptionsMonitor<WechatPayOptions>>,
}

impl WechatPayProviderFactory {
    /// Build the factory over an options monitor.
    pub fn new(
        monitor: Arc<dyn webx_core::options_monitor::IOptionsMonitor<WechatPayOptions>>,
    ) -> Self {
        Self { monitor }
    }
}

#[async_trait]
impl IPaymentProviderFactory for WechatPayProviderFactory {
    fn provider(&self) -> ProviderId {
        PROVIDER
    }

    async fn resolve(
        &self,
        _scope: &PaymentScope,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        let options = self.monitor.current();
        let provider = WechatPayProvider::new((*options).clone());
        provider.validate()?;
        Ok(Arc::new(provider))
    }
}

/// Builder used by custom [`IPaymentServiceFactory`](webx_pay::IPaymentServiceFactory)
/// implementations.
#[derive(Debug, Default, Clone, Copy)]
pub struct WechatPayProviderBuilder;

impl IPaymentProviderBuilder for WechatPayProviderBuilder {
    fn provider(&self) -> ProviderId {
        PROVIDER
    }

    fn build(
        &self,
        config: &PaymentProviderConfig,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        let options: WechatPayOptions =
            serde_json::from_value(config.settings.clone()).map_err(|error| {
                PaymentError::config(
                    PROVIDER,
                    format!("invalid WeChat Pay provider settings: {error}"),
                )
            })?;
        let provider = WechatPayProvider::new(options);
        provider.validate()?;
        Ok(Arc::new(provider))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;
    use webx_pay::Money;

    fn account() -> Arc<WechatPayAccount> {
        let options = test_support::options();
        let accounts = options.build_accounts().unwrap();
        Arc::clone(accounts.resolve(None).unwrap())
    }

    fn provider() -> WechatPayProvider {
        WechatPayProvider::new(test_support::options())
    }

    fn create(method: PaymentMethod, order_id: &str) -> CreatePayment {
        let flow = match method {
            PaymentMethod::WechatH5 => webx_pay::PaymentFlow::Redirect {
                success_url: "https://example.com/ok".to_string(),
                cancel_url: None,
            },
            PaymentMethod::WechatNative => webx_pay::PaymentFlow::QrCode,
            _ => webx_pay::PaymentFlow::App { return_url: None },
        };
        CreatePayment::new(order_id, Money::cny(9900), method, flow)
            .with_description("Integration test order")
    }

    #[test]
    fn provider_exposes_the_single_account_and_capabilities() {
        let provider = provider();
        assert_eq!(provider.id(), PROVIDER);
        assert_eq!(provider.accounts().len(), 1);
        assert_eq!(provider.accounts()[0].id.as_str(), AccountId::DEFAULT);
        let capabilities = provider.capabilities();
        assert!(capabilities.create && capabilities.query && capabilities.cancel);
        assert!(capabilities.refund && capabilities.partial_refund);
        assert!(capabilities.qr && capabilities.app && capabilities.hosted_checkout);
        assert!(!capabilities.capture);
        assert_eq!(capabilities.webhook, WebhookCapability::Rsa2Aes256Gcm);
        assert!(capabilities.supports_currency(&Currency::CNY));
        assert!(!capabilities.supports_currency(&Currency::USD));
    }

    #[test]
    fn validate_accepts_generated_configuration_and_reports_bad_configuration() {
        let provider = provider();
        assert!(provider.validate().is_ok());
        assert_eq!(provider.health().status, "pass");
        let mut bad = test_support::options();
        bad.accounts[0].api_v3_key = String::new();
        let broken = WechatPayProvider::new(bad);
        assert!(broken.validate().is_err());
        assert_eq!(broken.health().status, "fail");
        assert!(broken.accounts().is_empty());
    }

    #[test]
    fn account_debug_never_prints_secrets() {
        let text = format!("{:?}", account());
        assert!(!text.contains(&test_support::key_pair().private_pem));
        assert!(!text.contains(&test_support::api_v3_key_text()));
        assert!(text.contains("<redacted>"));
    }

    #[test]
    fn create_request_validation_rejects_other_currencies_manual_capture_and_methods() {
        let mut request = create(PaymentMethod::WechatNative, "ORDER-1");
        request.amount = Money::usd(100);
        assert!(matches!(
            validate_create_request(&request),
            Err(PaymentError::Validation { .. })
        ));

        let request = create(PaymentMethod::WechatNative, "ORDER-1").with_manual_capture();
        assert!(matches!(
            validate_create_request(&request),
            Err(PaymentError::Unsupported { .. })
        ));

        let request = create(PaymentMethod::Card, "ORDER-1");
        assert!(matches!(
            validate_create_request(&request),
            Err(PaymentError::Unsupported { .. })
        ));

        let request = create(PaymentMethod::WechatNative, "bad order id");
        assert!(matches!(
            validate_create_request(&request),
            Err(PaymentError::Validation { .. })
        ));
    }

    #[test]
    fn native_body_has_the_documented_shape() {
        let account = account();
        let request = create(PaymentMethod::WechatNative, "ORDER-1")
            .with_expires_at(1_582_934_400)
            .with_metadata("attach", "tenant-42");
        let body = build_create_body(
            &account,
            &request,
            Channel::Native,
            "https://example.com/notify",
        )
        .unwrap();
        assert_eq!(body["appid"], account.app_id);
        assert_eq!(body["mchid"], account.mch_id);
        assert_eq!(body["description"], "Integration test order");
        assert_eq!(body["out_trade_no"], "ORDER-1");
        assert_eq!(body["notify_url"], "https://example.com/notify");
        assert_eq!(body["amount"]["total"], 9900);
        assert_eq!(body["amount"]["currency"], "CNY");
        assert_eq!(body["time_expire"], "2020-02-29T08:00:00+08:00");
        assert_eq!(body["attach"], "tenant-42");
        assert!(body.get("payer").is_none());
    }

    #[test]
    fn body_description_defaults_and_length_is_checked() {
        let account = account();
        let mut request = create(PaymentMethod::WechatNative, "ORDER-1");
        request.description = String::new();
        let body =
            build_create_body(&account, &request, Channel::Native, "https://e.com/n").unwrap();
        assert_eq!(body["description"], "Order ORDER-1");

        request.description = "x".repeat(128);
        let error =
            build_create_body(&account, &request, Channel::Native, "https://e.com/n").unwrap_err();
        assert!(matches!(error, PaymentError::Validation { .. }));
    }

    #[test]
    fn jsapi_body_requires_and_carries_the_payer_openid() {
        let account = account();
        let request = create(PaymentMethod::WechatJsapi, "ORDER-1");
        let error =
            build_create_body(&account, &request, Channel::Jsapi, "https://e.com/n").unwrap_err();
        assert!(matches!(error, PaymentError::Validation { .. }));

        let request = request.with_payer_ref("oUpF8uMuAJO_M2pxb1Q9zNjWeS6o");
        let body =
            build_create_body(&account, &request, Channel::Jsapi, "https://e.com/n").unwrap();
        assert_eq!(body["payer"]["openid"], "oUpF8uMuAJO_M2pxb1Q9zNjWeS6o");
    }

    #[test]
    fn h5_body_requires_the_client_ip_and_h5_scene_info() {
        let account = account();
        let request = create(PaymentMethod::WechatH5, "ORDER-1");
        let error =
            build_create_body(&account, &request, Channel::H5, "https://e.com/n").unwrap_err();
        assert!(matches!(error, PaymentError::Validation { .. }));

        let request = request.with_client_ip("203.0.113.10".parse().unwrap());
        let body = build_create_body(&account, &request, Channel::H5, "https://e.com/n").unwrap();
        assert_eq!(body["scene_info"]["payer_client_ip"], "203.0.113.10");
        assert_eq!(body["scene_info"]["h5_info"]["type"], "Wap");
    }

    #[test]
    fn non_h5_body_includes_scene_info_when_the_client_ip_is_known() {
        let account = account();
        let request = create(PaymentMethod::WechatNative, "ORDER-1")
            .with_client_ip("203.0.113.10".parse().unwrap());
        let body =
            build_create_body(&account, &request, Channel::Native, "https://e.com/n").unwrap();
        assert_eq!(body["scene_info"]["payer_client_ip"], "203.0.113.10");
    }

    #[test]
    fn native_response_maps_to_a_qr_client_action() {
        let account = account();
        let value = json!({ "code_url": "weixin://wxpay/bizpayurl/up?pr=NwY5Mz9&groupid=00" });
        let action = client_action_for(Channel::Native, &account, &value).unwrap();
        assert_eq!(
            action,
            ClientAction::QrCode {
                content: "weixin://wxpay/bizpayurl/up?pr=NwY5Mz9&groupid=00".to_string()
            }
        );
        assert!(client_action_for(Channel::Native, &account, &json!({})).is_err());
    }

    #[test]
    fn h5_response_maps_to_a_redirect_client_action() {
        let account = account();
        let value =
            json!({ "h5_url": "https://wx.tenpay.com/cgi-bin/mmpayweb-bin/checkmweb?prepay_id=x" });
        let action = client_action_for(Channel::H5, &account, &value).unwrap();
        assert_eq!(
            action,
            ClientAction::Redirect {
                url: "https://wx.tenpay.com/cgi-bin/mmpayweb-bin/checkmweb?prepay_id=x".to_string()
            }
        );
    }

    #[test]
    fn app_response_carries_verified_sdk_parameters() {
        let account = account();
        let value = json!({ "prepay_id": "wx201410272009395522657a690389285100" });
        let action = client_action_for(Channel::App, &account, &value).unwrap();
        let ClientAction::AppOrder { provider, payload } = action else {
            panic!("expected AppOrder");
        };
        assert_eq!(provider, PROVIDER);
        assert_eq!(payload["appId"], account.app_id);
        assert_eq!(payload["partnerId"], account.mch_id);
        assert_eq!(payload["prepayId"], "wx201410272009395522657a690389285100");
        assert_eq!(payload["packageValue"], "Sign=WXPay");
        let message = app_client_signature_message(
            &account.app_id,
            payload["timeStamp"].as_str().unwrap(),
            payload["nonceStr"].as_str().unwrap(),
            payload["prepayId"].as_str().unwrap(),
        );
        crate::sign::verify_message_base64(
            &account.signing_key.to_public_key(),
            &message,
            payload["sign"].as_str().unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn jsapi_response_carries_verified_sdk_parameters() {
        let account = account();
        let value = json!({ "prepay_id": "wx201410272009395522657a690389285100" });
        let action = client_action_for(Channel::Jsapi, &account, &value).unwrap();
        let ClientAction::AppOrder { provider, payload } = action else {
            panic!("expected AppOrder");
        };
        assert_eq!(provider, PROVIDER);
        assert_eq!(
            payload["package"],
            "prepay_id=wx201410272009395522657a690389285100"
        );
        assert_eq!(payload["signType"], "RSA");
        let message = jsapi_client_signature_message(
            &account.app_id,
            payload["timeStamp"].as_str().unwrap(),
            payload["nonceStr"].as_str().unwrap(),
            payload["package"].as_str().unwrap(),
        );
        crate::sign::verify_message_base64(
            &account.signing_key.to_public_key(),
            &message,
            payload["paySign"].as_str().unwrap(),
        )
        .unwrap();
        assert!(client_action_for(Channel::Jsapi, &account, &json!({})).is_err());
    }

    #[test]
    fn query_response_maps_trade_state_amount_and_ids() {
        let account = account();
        let value = json!({
            "appid": "wxd678efh567hg6787",
            "mchid": "1900000001",
            "out_trade_no": "ORDER-1",
            "transaction_id": "4200001213202301010000000000",
            "trade_type": "NATIVE",
            "trade_state": "SUCCESS",
            "trade_state_desc": "支付成功",
            "attach": "tenant-42",
            "success_time": "2018-06-08T10:34:56+08:00",
            "payer": { "openid": "oUpF8uMuAJO_M2pxb1Q9zNjWeS6o" },
            "amount": { "total": 9900, "payer_total": 9800, "currency": "CNY" },
        });
        let intent = parse_trade(&value, &account, "ORDER-1").unwrap();
        assert_eq!(intent.provider, PROVIDER);
        assert_eq!(intent.account.as_str(), AccountId::DEFAULT);
        assert_eq!(intent.order_id, "ORDER-1");
        assert_eq!(intent.status, PaymentStatus::Succeeded);
        assert_eq!(intent.provider_status, "SUCCESS");
        assert_eq!(
            intent.provider_payment_id.as_deref(),
            Some("4200001213202301010000000000")
        );
        assert_eq!(intent.amount, Money::cny(9900));
        assert_eq!(intent.amount_captured, Some(Money::cny(9900)));
        assert_eq!(intent.created_at, 1_528_425_296);
        assert_eq!(
            intent.metadata.get("attach").map(String::as_str),
            Some("tenant-42")
        );
    }

    #[test]
    fn query_response_maps_non_paid_states() {
        let account = account();
        for (trade_state, expected) in [
            ("NOTPAY", PaymentStatus::RequiresAction),
            ("USERPAYING", PaymentStatus::Processing),
            ("CLOSED", PaymentStatus::Canceled),
            ("REVOKED", PaymentStatus::Canceled),
            ("PAYERROR", PaymentStatus::Failed),
            ("REFUND", PaymentStatus::Refunded),
            ("FUTURE_STATE", PaymentStatus::Unknown),
        ] {
            let value = json!({ "trade_state": trade_state, "amount": { "total": 100 } });
            let intent = parse_trade(&value, &account, "ORDER-1").unwrap();
            assert_eq!(intent.status, expected, "{trade_state}");
            assert_eq!(intent.provider_status, trade_state);
        }
        let error =
            parse_trade(&json!({ "amount": { "total": 1 } }), &account, "ORDER-1").unwrap_err();
        assert!(matches!(error, PaymentError::Serialization(_)));
        let error =
            parse_trade(&json!({ "trade_state": "SUCCESS" }), &account, "ORDER-1").unwrap_err();
        assert!(matches!(error, PaymentError::Serialization(_)));
    }

    #[test]
    fn refund_response_maps_status_amount_and_ids() {
        let account = account();
        let value = json!({
            "refund_id": "50000000382019052709732678859",
            "out_refund_no": "REFUND-1",
            "transaction_id": "4200001213202301010000000000",
            "out_trade_no": "ORDER-1",
            "channel": "ORIGINAL",
            "user_received_account": "招商银行信用卡",
            "success_time": "2020-12-01T16:18:12+08:00",
            "create_time": "2020-12-01T16:18:12+08:00",
            "status": "SUCCESS",
            "funds_account": "UNSETTLED",
            "amount": {
                "total": 9900,
                "refund": 1900,
                "payer_total": 9900,
                "payer_refund": 1900,
                "currency": "CNY"
            }
        });
        let refund = parse_refund(&value, &account, None, None).unwrap();
        assert_eq!(refund.provider, PROVIDER);
        assert_eq!(refund.order_id, "ORDER-1");
        assert_eq!(refund.refund_id, "REFUND-1");
        assert_eq!(
            refund.provider_refund_id.as_deref(),
            Some("50000000382019052709732678859")
        );
        assert_eq!(refund.status, webx_pay::RefundStatus::Succeeded);
        assert_eq!(refund.provider_status, "SUCCESS");
        assert_eq!(refund.amount, Money::cny(1900));
        assert_eq!(refund.created_at, 1_606_810_692);
        assert_eq!(refund.succeeded_at, Some(1_606_810_692));
    }

    #[test]
    fn refund_status_mapping_covers_processing_and_abnormal() {
        let account = account();
        for (status, expected) in [
            ("PROCESSING", webx_pay::RefundStatus::Pending),
            ("ABNORMAL", webx_pay::RefundStatus::Failed),
            ("CLOSED", webx_pay::RefundStatus::Canceled),
            ("FUTURE", webx_pay::RefundStatus::Unknown),
        ] {
            let value = json!({
                "refund_id": "1",
                "out_refund_no": "R-1",
                "out_trade_no": "ORDER-1",
                "status": status,
                "amount": { "refund": 100 },
            });
            let refund = parse_refund(&value, &account, None, None).unwrap();
            assert_eq!(refund.status, expected, "{status}");
        }
        let value = json!({ "refund_id": "1", "status": "SUCCESS", "amount": { "refund": 1 } });
        let refund = parse_refund(&value, &account, Some("ORDER-9"), Some("R-9")).unwrap();
        assert_eq!(refund.order_id, "ORDER-9");
        assert_eq!(refund.refund_id, "R-9");
        let bad_currency = json!({
            "refund_id": "1",
            "status": "SUCCESS",
            "amount": { "refund": 1, "currency": "USD" },
        });
        assert!(parse_refund(&bad_currency, &account, Some("ORDER-9"), Some("R-9")).is_err());
    }

    #[test]
    fn merchant_ids_are_validated_before_they_reach_a_signed_url() {
        assert_eq!(validate_trade_no("ORDER_1-a|b*").unwrap(), "ORDER_1-a|b*");
        assert!(validate_trade_no("").is_err());
        assert!(validate_trade_no("ORDER/1").is_err());
        assert!(validate_trade_no(&"x".repeat(33)).is_err());
        assert_eq!(validate_refund_no("REFUND_1@x").unwrap(), "REFUND_1@x");
        assert!(validate_refund_no("REFUND/1").is_err());
        assert!(validate_refund_no(&"x".repeat(65)).is_err());
    }

    #[test]
    fn notify_url_resolution_prefers_request_override_then_account_default() {
        let account = account();
        assert_eq!(
            account
                .resolve_notify_url(Some("https://example.com/override"))
                .unwrap(),
            "https://example.com/override"
        );
        assert_eq!(
            account.resolve_notify_url(None).unwrap(),
            "https://example.com/webhooks/payments/wechatpay"
        );
        let mut silent = test_support::options();
        silent.accounts[0].notify_url = String::new();
        let silent_accounts = silent.build_accounts().unwrap();
        let silent_account = silent_accounts.resolve(None).unwrap();
        let error = silent_account.resolve_notify_url(None).unwrap_err();
        assert!(matches!(error, PaymentError::Config { .. }));
        let error = account
            .resolve_notify_url(Some("http://example.com/hook"))
            .unwrap_err();
        assert!(matches!(error, PaymentError::Validation { .. }));
    }

    #[test]
    fn unknown_account_is_a_validation_error() {
        let provider = provider();
        assert!(provider
            .account_for(Some(&AccountId::from("missing")))
            .is_err());
    }

    #[tokio::test]
    async fn refresh_platform_certificates_is_a_noop_for_public_key_mode() {
        let account = test_support::account_options().with_public_key(
            "PUB_KEY_ID_TEST",
            test_support::key_pair().public_pem.clone(),
        );
        let provider = WechatPayProvider::new(WechatPayOptions::new(account));
        provider.refresh_platform_certificates().await.unwrap();
    }

    #[tokio::test]
    async fn wechatpay_satisfies_the_payment_provider_contract() {
        webx_pay::testing::payment_provider_contract(&provider()).await;
    }

    /// Settings JSON with the same shape as [`WechatPayOptions`].
    fn provider_settings() -> Value {
        json!({
            "accounts": [{
                "mch_id": "1900000001",
                "app_id": "wxd678efh567hg6787",
                "serial_no": "5157F09EFDC096DE15EBE81A47057A7232F1B8E1",
                "private_key_pem": test_support::key_pair().private_pem.clone(),
                "api_v3_key": test_support::api_v3_key_text(),
                "notify_url": "https://example.com/webhooks/payments/wechatpay",
            }],
        })
    }

    #[test]
    fn provider_builder_parses_settings_and_rejects_invalid_ones() {
        let config = PaymentProviderConfig::new(PROVIDER, provider_settings());
        let provider = WechatPayProviderBuilder.build(&config).unwrap();
        assert_eq!(provider.id(), PROVIDER);
        assert_eq!(provider.accounts().len(), 1);

        let empty = PaymentProviderConfig::new(PROVIDER, json!({}));
        assert!(matches!(
            WechatPayProviderBuilder.build(&empty),
            Err(PaymentError::Config { .. })
        ));
    }

    #[tokio::test]
    async fn provider_factory_resolves_a_validated_provider_from_the_monitor() {
        let monitor = Arc::new(webx_core::options_monitor::OptionsMonitor::new(
            test_support::options(),
        ));
        let factory = WechatPayProviderFactory::new(monitor);
        let provider = factory.resolve(&PaymentScope::new(PROVIDER)).await.unwrap();
        assert_eq!(provider.id(), PROVIDER);
        assert!(provider.validate().is_ok());
    }

    #[test]
    fn payment_options_expose_four_channels_per_account() {
        let provider = provider();
        let options = provider.payment_options();
        assert_eq!(options.len(), 4);
        let expected = [
            (PaymentMethod::WechatNative, PaymentFlowKind::QrCode),
            (PaymentMethod::WechatJsapi, PaymentFlowKind::ClientSdk),
            (PaymentMethod::WechatApp, PaymentFlowKind::App),
            (PaymentMethod::WechatH5, PaymentFlowKind::Redirect),
        ];
        for (method, flow) in expected {
            let option = options
                .iter()
                .find(|option| option.method == method)
                .unwrap_or_else(|| panic!("missing frontend option for {method:?}"));
            assert_eq!(option.provider, PROVIDER);
            assert_eq!(option.account.as_str(), AccountId::DEFAULT);
            assert_eq!(option.flows, vec![flow]);
        }
    }

    #[test]
    fn payment_options_cover_every_configured_account() {
        let options =
            test_support::options().with_account(test_support::account_options().with_id("second"));
        let provider = WechatPayProvider::new(options);
        let catalog = provider.payment_options();
        assert_eq!(catalog.len(), 8);
        for id in [AccountId::DEFAULT, "second"] {
            let count = catalog
                .iter()
                .filter(|option| option.account.as_str() == id)
                .count();
            assert_eq!(count, 4, "account `{id}` must have four channels");
        }
    }

    #[test]
    fn payment_options_carry_only_the_public_app_id() {
        let app_id = test_support::account_options().app_id;
        for option in provider().payment_options() {
            let config = option.client_config.expect("client_config");
            assert_eq!(
                config.as_object().map(|object| object.len()),
                Some(1),
                "client_config must contain only the public app_id"
            );
            assert_eq!(config["app_id"], json!(app_id));
        }
    }

    #[test]
    fn payment_options_serialize_without_secret_material() {
        let serialized = serde_json::to_string(&provider().payment_options()).unwrap();
        let secrets = [
            test_support::key_pair().private_pem.clone(),
            test_support::api_v3_key_text(),
            test_support::account_options().serial_no,
        ];
        for secret in secrets {
            assert!(!serialized.contains(&secret));
        }
        for field in ["private_key_pem", "api_v3_key", "serial_no", "signing_key"] {
            assert!(
                !serialized.contains(field),
                "secret field `{field}` leaked into the catalog"
            );
        }
        assert!(serialized.contains(&test_support::account_options().app_id));
    }

    #[test]
    fn payment_options_forward_the_public_account_label() {
        let credentials = account();
        let info = PaymentAccount {
            id: AccountId::default_account(),
            provider: PROVIDER,
            live: true,
            label: Some("WeChat Pay main".to_string()),
        };
        let accounts = AccountSet::new(
            PROVIDER,
            info.id.clone(),
            [(info.id.clone(), (*credentials).clone(), info)],
        )
        .unwrap();
        let mut provider = provider();
        provider.accounts = Ok(accounts);
        let options = provider.payment_options();
        assert_eq!(options.len(), 4);
        assert!(options
            .iter()
            .all(|option| option.label.as_deref() == Some("WeChat Pay main")));
    }

    #[test]
    fn payment_options_are_empty_when_configuration_is_invalid() {
        let mut options = test_support::options();
        options.accounts.clear();
        let broken = WechatPayProvider::new(options);
        assert!(broken.payment_options().is_empty());
    }
}
