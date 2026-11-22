//! Provider-neutral payment request/response models.

use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::PaymentError;
use crate::ids::{AccountId, ProviderId};
use crate::money::Money;
use crate::status::{PaymentStatus, RefundStatus};

/// Payment instrument/channel requested by the caller.
///
/// Adapters validate that the combination of provider + method + flow is
/// available and return `Unsupported` otherwise; the enum does not multiply
/// out per provider.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PaymentMethod {
    /// Card (the card data never reaches this server; hosted fields/SDK token).
    Card,
    /// Alipay desktop web redirect.
    AlipayPage,
    /// Alipay mobile web redirect.
    AlipayWap,
    /// Alipay app SDK order string.
    AlipayApp,
    /// Alipay QR (`precreate`).
    AlipayQr,
    /// WeChat Pay Native (QR).
    WechatNative,
    /// WeChat Pay JSAPI (official account / mini program, needs `payer_ref`).
    WechatJsapi,
    /// WeChat Pay App.
    WechatApp,
    /// WeChat Pay H5 (needs `client_ip`).
    WechatH5,
    /// PayPal wallet/order approval.
    PayPal,
    /// Apple Pay token, forwarded to the underlying acquirer adapter.
    ApplePay,
    /// Google Pay token, forwarded to the underlying acquirer adapter.
    GooglePay,
    /// Adapter-specific method name.
    Other(String),
}

impl PaymentMethod {
    /// Stable wire name.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Card => "card",
            Self::AlipayPage => "alipay_page",
            Self::AlipayWap => "alipay_wap",
            Self::AlipayApp => "alipay_app",
            Self::AlipayQr => "alipay_qr",
            Self::WechatNative => "wechat_native",
            Self::WechatJsapi => "wechat_jsapi",
            Self::WechatApp => "wechat_app",
            Self::WechatH5 => "wechat_h5",
            Self::PayPal => "paypal",
            Self::ApplePay => "apple_pay",
            Self::GooglePay => "google_pay",
            Self::Other(name) => name,
        }
    }

    /// Default display name for a checkout page.
    ///
    /// Applications may localize/override it through
    /// [`PaymentOption::with_display_name`](crate::PaymentOption::with_display_name).
    pub fn display_name(&self) -> &str {
        match self {
            Self::Card => "Card",
            Self::AlipayPage | Self::AlipayWap | Self::AlipayApp | Self::AlipayQr => "Alipay",
            Self::WechatNative | Self::WechatJsapi | Self::WechatApp | Self::WechatH5 => {
                "WeChat Pay"
            }
            Self::PayPal => "PayPal",
            Self::ApplePay => "Apple Pay",
            Self::GooglePay => "Google Pay",
            Self::Other(name) => name.as_str(),
        }
    }

    /// Default description for a checkout page.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Card => "Credit or debit card",
            Self::AlipayPage | Self::AlipayWap | Self::AlipayApp | Self::AlipayQr => {
                "Pay with Alipay"
            }
            Self::WechatNative | Self::WechatJsapi | Self::WechatApp | Self::WechatH5 => {
                "Pay with WeChat Pay"
            }
            Self::PayPal => "Pay with PayPal",
            Self::ApplePay => "Pay with Apple Pay",
            Self::GooglePay => "Pay with Google Pay",
            Self::Other(_) => "Pay with the selected method",
        }
    }

    /// Stable icon key the frontend maps to its own bundled asset.
    ///
    /// The framework deliberately does not ship image binaries; the key is
    /// stable across providers (`card`, `alipay`, `wechatpay`, `paypal`,
    /// `apple_pay`, `google_pay`).
    pub fn icon_key(&self) -> &str {
        match self {
            Self::Card => "card",
            Self::AlipayPage | Self::AlipayWap | Self::AlipayApp | Self::AlipayQr => "alipay",
            Self::WechatNative | Self::WechatJsapi | Self::WechatApp | Self::WechatH5 => {
                "wechatpay"
            }
            Self::PayPal => "paypal",
            Self::ApplePay => "apple_pay",
            Self::GooglePay => "google_pay",
            Self::Other(name) => name.as_str(),
        }
    }
}

impl fmt::Display for PaymentMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PaymentMethod {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let lower = value.trim().to_ascii_lowercase();
        Ok(match lower.as_str() {
            "card" => Self::Card,
            "alipay_page" => Self::AlipayPage,
            "alipay_wap" => Self::AlipayWap,
            "alipay_app" => Self::AlipayApp,
            "alipay_qr" | "alipay_precreate" => Self::AlipayQr,
            "wechat_native" | "wechat_qr" => Self::WechatNative,
            "wechat_jsapi" => Self::WechatJsapi,
            "wechat_app" => Self::WechatApp,
            "wechat_h5" => Self::WechatH5,
            "paypal" => Self::PayPal,
            "apple_pay" => Self::ApplePay,
            "google_pay" => Self::GooglePay,
            _ => Self::Other(value.trim().to_string()),
        })
    }
}

impl Serialize for PaymentMethod {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PaymentMethod {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MethodVisitor;
        impl Visitor<'_> for MethodVisitor {
            type Value = PaymentMethod;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a payment method name")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                PaymentMethod::from_str(value).map_err(E::custom)
            }
        }
        deserializer.deserialize_str(MethodVisitor)
    }
}

/// How the payer completes the payment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum PaymentFlow {
    /// Server creates an intent; the client SDK confirms it.
    ClientSdk {
        /// Where the provider should send the payer afterwards.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        return_url: Option<String>,
    },
    /// Redirect the browser to a provider-hosted page.
    Redirect {
        /// Success redirect.
        success_url: String,
        /// Cancel redirect.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cancel_url: Option<String>,
    },
    /// Render a QR code to the payer.
    QrCode,
    /// Open the provider's app SDK.
    App {
        /// Where the provider should return the app afterwards.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        return_url: Option<String>,
    },
}

/// What the client must do next after a payment was created.
///
/// Provider-specific payloads are preserved verbatim in `payload` instead of
/// pretending that `client_secret`, `prepay_id` and `orderStr` share a shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ClientAction {
    /// Send the browser to `url`.
    Redirect {
        /// Absolute provider URL.
        url: String,
    },
    /// Render `content` as a QR code.
    QrCode {
        /// QR payload (URL or opaque provider string).
        content: String,
    },
    /// Pass `payload` to the provider's app SDK (Alipay `orderStr`, WeChat
    /// prepay parameters, ...).
    AppOrder {
        /// Provider the payload belongs to.
        provider: ProviderId,
        /// Provider-specific SDK parameters.
        payload: serde_json::Value,
    },
    /// Pass `payload` to the provider's client-side SDK (Stripe Elements,
    /// PayPal JS SDK, ...).
    ClientSdk {
        /// Provider the payload belongs to.
        provider: ProviderId,
        /// Provider-specific client parameters.
        payload: serde_json::Value,
    },
}

/// Whether the provider captures funds immediately or after an explicit
/// capture call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    /// Capture as part of the payment (the default).
    #[default]
    Automatic,
    /// Authorize now, capture later with `IPaymentProvider::capture_payment`.
    Manual,
}

/// A request to create a payment.
#[derive(Debug, Clone)]
pub struct CreatePayment {
    /// Merchant order id (the idempotency anchor for every provider).
    pub order_id: String,
    /// Amount to charge.
    pub amount: Money,
    /// Instrument/channel.
    pub method: PaymentMethod,
    /// Client-side completion flow.
    pub flow: PaymentFlow,
    /// Capture immediately or after an explicit capture call.
    pub capture: CaptureMode,
    /// Human-readable description shown by the provider.
    pub description: String,
    /// Provider metadata / attach data (bounded by the caller).
    pub metadata: BTreeMap<String, String>,
    /// Caller idempotency key; adapters derive a stable one from `order_id`
    /// when this is absent.
    pub idempotency_key: Option<String>,
    /// Payer reference for providers that need one (WeChat JSAPI `openid`,
    /// Alipay `buyer_id`).
    pub payer_ref: Option<String>,
    /// Client IP for providers that require it (WeChat H5).
    pub client_ip: Option<IpAddr>,
    /// Per-request callback URL override.
    pub notify_url: Option<String>,
    /// Expiry as Unix seconds.
    pub expires_at: Option<i64>,
}

impl CreatePayment {
    /// Build a payment request.
    pub fn new(
        order_id: impl Into<String>,
        amount: Money,
        method: PaymentMethod,
        flow: PaymentFlow,
    ) -> Self {
        Self {
            order_id: order_id.into(),
            amount,
            method,
            flow,
            capture: CaptureMode::Automatic,
            description: String::new(),
            metadata: BTreeMap::new(),
            idempotency_key: None,
            payer_ref: None,
            client_ip: None,
            notify_url: None,
            expires_at: None,
        }
    }

    /// Set the provider-visible description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Authorize now and capture explicitly later.
    pub fn with_manual_capture(mut self) -> Self {
        self.capture = CaptureMode::Manual;
        self
    }

    /// Add one metadata entry.
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Set an explicit idempotency key (otherwise `order_id` is used).
    pub fn with_idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    /// Payer reference (openid/buyer id).
    pub fn with_payer_ref(mut self, payer_ref: impl Into<String>) -> Self {
        self.payer_ref = Some(payer_ref.into());
        self
    }

    /// Client IP for H5 payments.
    pub fn with_client_ip(mut self, client_ip: IpAddr) -> Self {
        self.client_ip = Some(client_ip);
        self
    }

    /// Override the configured callback URL for this request.
    pub fn with_notify_url(mut self, notify_url: impl Into<String>) -> Self {
        self.notify_url = Some(notify_url.into());
        self
    }

    /// Expiry timestamp (Unix seconds).
    pub fn with_expires_at(mut self, expires_at: i64) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    /// Basic request validation shared by all adapters.
    pub fn validate(&self) -> Result<(), PaymentError> {
        if self.order_id.trim().is_empty() {
            return Err(PaymentError::validation("order_id must not be empty"));
        }
        if self.order_id.len() > 64 {
            return Err(PaymentError::validation(
                "order_id must be at most 64 characters",
            ));
        }
        if self.order_id.chars().any(char::is_control) {
            return Err(PaymentError::validation(
                "order_id must not contain control characters",
            ));
        }
        if self.amount.minor <= 0 {
            return Err(PaymentError::validation("amount must be positive"));
        }
        if self.metadata.len() > 32 {
            return Err(PaymentError::validation(
                "at most 32 metadata entries are supported",
            ));
        }
        if self
            .metadata
            .iter()
            .any(|(key, value)| key.len() > 64 || value.len() > 512)
        {
            return Err(PaymentError::validation(
                "metadata keys must be <= 64 and values <= 512 characters",
            ));
        }
        Ok(())
    }
}

/// A created payment, normalized across providers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaymentIntent {
    /// Provider that owns the payment.
    pub provider: ProviderId,
    /// Merchant account used.
    pub account: AccountId,
    /// Merchant order id.
    pub order_id: String,
    /// Provider-side payment id (`pi_...`, `trade_no`, `transaction_id`, ...).
    pub provider_payment_id: Option<String>,
    /// Normalized status.
    pub status: PaymentStatus,
    /// Raw provider status.
    pub provider_status: String,
    /// Requested amount.
    pub amount: Money,
    /// Captured amount, when the provider reports it.
    pub amount_captured: Option<Money>,
    /// Refunded amount, when the provider reports it.
    pub amount_refunded: Option<Money>,
    /// Next client action, when the payer must do something.
    pub client_action: Option<ClientAction>,
    /// Creation time (Unix seconds).
    pub created_at: i64,
    /// Expiry (Unix seconds), when the provider reports it.
    pub expires_at: Option<i64>,
    /// Provider metadata echoed back.
    pub metadata: BTreeMap<String, String>,
    /// Provider request id for support.
    pub request_id: Option<String>,
}

impl PaymentIntent {
    /// Build a normalized payment intent.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: ProviderId,
        account: AccountId,
        order_id: impl Into<String>,
        amount: Money,
        status: PaymentStatus,
        provider_status: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            account,
            order_id: order_id.into(),
            provider_payment_id: None,
            status,
            provider_status: provider_status.into(),
            amount,
            amount_captured: None,
            amount_refunded: None,
            client_action: None,
            created_at: 0,
            expires_at: None,
            metadata: BTreeMap::new(),
            request_id: None,
        }
    }
}

/// A refund request.
#[derive(Debug, Clone)]
pub struct RefundCommand {
    /// Merchant order id being refunded.
    pub order_id: String,
    /// Merchant refund id (the idempotency anchor for the refund).
    pub refund_id: String,
    /// Amount to refund; `None` refunds the full captured amount.
    pub amount: Option<Money>,
    /// Merchant reason shown to operators.
    pub reason: String,
    /// Per-request callback URL override.
    pub notify_url: Option<String>,
    /// Caller idempotency key; defaults to `refund_id`.
    pub idempotency_key: Option<String>,
}

impl RefundCommand {
    /// Build a full refund when `amount` is `None`.
    pub fn new(order_id: impl Into<String>, refund_id: impl Into<String>) -> Self {
        Self {
            order_id: order_id.into(),
            refund_id: refund_id.into(),
            amount: None,
            reason: String::new(),
            notify_url: None,
            idempotency_key: None,
        }
    }

    /// Refund a specific amount.
    pub fn with_amount(mut self, amount: Money) -> Self {
        self.amount = Some(amount);
        self
    }

    /// Set the merchant reason.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = reason.into();
        self
    }

    /// Override the callback URL.
    pub fn with_notify_url(mut self, notify_url: impl Into<String>) -> Self {
        self.notify_url = Some(notify_url.into());
        self
    }

    /// Set an explicit idempotency key.
    pub fn with_idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    /// Shared validation.
    pub fn validate(&self) -> Result<(), PaymentError> {
        if self.order_id.trim().is_empty() {
            return Err(PaymentError::validation("order_id must not be empty"));
        }
        if self.refund_id.trim().is_empty() {
            return Err(PaymentError::validation("refund_id must not be empty"));
        }
        if let Some(amount) = &self.amount {
            if amount.minor <= 0 {
                return Err(PaymentError::validation("refund amount must be positive"));
            }
        }
        Ok(())
    }
}

/// A normalized refund.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refund {
    /// Provider that owns the refund.
    pub provider: ProviderId,
    /// Merchant account used.
    pub account: AccountId,
    /// Merchant order id.
    pub order_id: String,
    /// Merchant refund id.
    pub refund_id: String,
    /// Provider refund id.
    pub provider_refund_id: Option<String>,
    /// Normalized status.
    pub status: RefundStatus,
    /// Raw provider status.
    pub provider_status: String,
    /// Refunded amount.
    pub amount: Money,
    /// Creation time (Unix seconds).
    pub created_at: i64,
    /// Completion time, when known.
    pub succeeded_at: Option<i64>,
    /// Provider request id.
    pub request_id: Option<String>,
}

/// Public description of a merchant account served by an adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentAccount {
    /// Account id used in requests and callback routes.
    pub id: AccountId,
    /// Owning provider.
    pub provider: ProviderId,
    /// Whether the account uses live credentials (vs test/sandbox).
    pub live: bool,
    /// Optional operator label.
    pub label: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    fn payment() -> CreatePayment {
        CreatePayment::new(
            "ORDER-1",
            Money::new(Currency::CNY, 9900),
            PaymentMethod::Card,
            PaymentFlow::ClientSdk {
                return_url: Some("https://example.com/ok".into()),
            },
        )
    }

    #[test]
    fn rejects_empty_order_and_non_positive_amount() {
        let mut request = payment();
        request.order_id = "  ".into();
        assert!(request.validate().is_err());

        let mut request = payment();
        request.amount = Money::new(Currency::CNY, 0);
        assert!(request.validate().is_err());
    }

    #[test]
    fn accepts_typical_request() {
        assert!(payment()
            .with_metadata("user_id", "42")
            .with_idempotency_key("idem-1")
            .validate()
            .is_ok());
    }

    #[test]
    fn method_serde_roundtrips_unknown_names() {
        let method: PaymentMethod = serde_json::from_str("\"alipay_qr\"").unwrap();
        assert_eq!(method, PaymentMethod::AlipayQr);
        let method: PaymentMethod = serde_json::from_str("\"acme_wallet\"").unwrap();
        assert_eq!(method, PaymentMethod::Other("acme_wallet".into()));
        assert_eq!(
            serde_json::to_string(&PaymentMethod::WechatJsapi).unwrap(),
            "\"wechat_jsapi\""
        );
    }

    #[test]
    fn flow_and_action_are_tagged() {
        let flow = serde_json::to_string(&PaymentFlow::QrCode).unwrap();
        assert_eq!(flow, "{\"type\":\"qr_code\"}");
        let action: ClientAction =
            serde_json::from_str("{\"type\":\"qr_code\",\"content\":\"weixin://x\"}").unwrap();
        assert_eq!(
            action,
            ClientAction::QrCode {
                content: "weixin://x".into()
            }
        );
    }
}
