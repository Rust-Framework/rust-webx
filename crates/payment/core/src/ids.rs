//! Stable provider and account identifiers.

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A payment provider understood by the domain kit.
///
/// The variants are a closed taxonomy: an adapter owns exactly one provider id
/// and the hub routes by it. `Custom` exists for third-party adapters that
/// implement the same ports; built-in behaviour is never selected by string
/// matching on arbitrary input.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProviderId {
    /// Stripe (cards, wallets, hosted Checkout).
    Stripe,
    /// Alipay (page/wap/app/precreate).
    Alipay,
    /// WeChat Pay APIv3 (Native/JSAPI/App/H5).
    WechatPay,
    /// PayPal Orders v2.
    PayPal,
    /// Adyen Checkout.
    Adyen,
    /// Square Payments.
    Square,
    /// Adapter supplied by the application or a third party.
    Custom(String),
}

impl ProviderId {
    /// Wire/config name (`"stripe"`, `"wechatpay"`, `"custom-name"`).
    pub fn as_str(&self) -> &str {
        match self {
            Self::Stripe => "stripe",
            Self::Alipay => "alipay",
            Self::WechatPay => "wechatpay",
            Self::PayPal => "paypal",
            Self::Adyen => "adyen",
            Self::Square => "square",
            Self::Custom(name) => name,
        }
    }

    /// Human-readable provider name for checkout UI.
    pub fn display_name(&self) -> &str {
        match self {
            Self::Stripe => "Stripe",
            Self::Alipay => "Alipay",
            Self::WechatPay => "WeChat Pay",
            Self::PayPal => "PayPal",
            Self::Adyen => "Adyen",
            Self::Square => "Square",
            Self::Custom(name) => name,
        }
    }

    /// All built-in providers, in documentation order.
    pub fn builtins() -> &'static [ProviderId] {
        &[
            Self::Stripe,
            Self::Alipay,
            Self::WechatPay,
            Self::PayPal,
            Self::Adyen,
            Self::Square,
        ]
    }

    /// Whether this is one of the built-in providers.
    pub fn is_builtin(&self) -> bool {
        !matches!(self, Self::Custom(_))
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parsing is case-insensitive and trims surrounding whitespace. Unknown names
/// become [`ProviderId::Custom`] instead of being rejected: a third-party
/// adapter may legitimately serve a name the framework does not know.
impl FromStr for ProviderId {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let name = value.trim();
        let lower = name.to_ascii_lowercase();
        Ok(match lower.as_str() {
            "stripe" => Self::Stripe,
            "alipay" => Self::Alipay,
            "wechatpay" | "wechat_pay" | "wechat-pay" | "wechat" => Self::WechatPay,
            "paypal" => Self::PayPal,
            "adyen" => Self::Adyen,
            "square" => Self::Square,
            _ => Self::Custom(name.to_string()),
        })
    }
}

impl Serialize for ProviderId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ProviderId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ProviderVisitor;
        impl Visitor<'_> for ProviderVisitor {
            type Value = ProviderId;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a payment provider name")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                ProviderId::from_str(value).map_err(E::custom)
            }
        }
        deserializer.deserialize_str(ProviderVisitor)
    }
}

/// Merchant account inside one provider (multi-account / service-provider mode).
///
/// One adapter instance can hold several accounts (different merchant ids,
/// signing keys and webhook secrets). Requests without an explicit account use
/// the provider's default account. Router-style use cases that need a different
/// account per tenant pass [`CreatePayment::account`](crate::CreatePayment)
/// or use the account-scoped webhook route.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountId(String);

impl AccountId {
    /// Name of the account used when a request does not select one.
    pub const DEFAULT: &'static str = "default";

    /// Create an account id.
    ///
    /// Empty ids and ids containing control characters or path separators are
    /// rejected: the id is used in callback routes and log fields.
    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.is_empty() {
            return Err("account id must not be empty");
        }
        if value.len() > 64 {
            return Err("account id must be at most 64 characters");
        }
        if value
            .chars()
            .any(|ch| ch.is_control() || ch == '/' || ch == '\\')
        {
            return Err("account id must not contain control characters or path separators");
        }
        Ok(Self(value))
    }

    /// The default account id (`"default"`).
    pub fn default_account() -> Self {
        Self(Self::DEFAULT.to_string())
    }

    /// Borrow the id.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is [`AccountId::DEFAULT`].
    pub fn is_default(&self) -> bool {
        self.0 == Self::DEFAULT
    }
}

impl Default for AccountId {
    fn default() -> Self {
        Self::default_account()
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for AccountId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for AccountId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<AccountId> for String {
    fn from(value: AccountId) -> Self {
        value.0
    }
}

/// Tenant identifier used by runtime payment configuration.
///
/// The value must come from the trusted identity (`IdentityUser::tenant_id`) or
/// server-side configuration, never from a request body/query.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantId(String);

impl TenantId {
    /// Validate and wrap a tenant id.
    ///
    /// Control characters are rejected because the value is used in log fields
    /// and cache keys; `/` is allowed (hierarchical tenant ids are common).
    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err("tenant id must not be empty");
        }
        if value.len() > 128 {
            return Err("tenant id must be at most 128 characters");
        }
        if value.chars().any(char::is_control) {
            return Err("tenant id must not contain control characters");
        }
        Ok(Self(value))
    }

    /// Borrow the id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for TenantId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for TenantId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_roundtrips_through_strings() {
        for provider in ProviderId::builtins() {
            let text = provider.as_str();
            let parsed: ProviderId = text.parse().unwrap();
            assert_eq!(&parsed, provider);
        }
        assert_eq!(
            "WECHAT".parse::<ProviderId>().unwrap(),
            ProviderId::WechatPay
        );
        assert_eq!(
            "acme-pay".parse::<ProviderId>().unwrap(),
            ProviderId::Custom("acme-pay".to_string())
        );
    }

    #[test]
    fn account_id_rejects_route_unsafe_values() {
        assert!(AccountId::new("").is_err());
        assert!(AccountId::new("a/b").is_err());
        assert!(AccountId::new("a\nb").is_err());
        assert_eq!(AccountId::new("acct_1").unwrap().as_str(), "acct_1");
        assert!(AccountId::default_account().is_default());
    }
}
