//! Declared provider capabilities (what an adapter can actually do).

use crate::money::Currency;
use crate::status::PaymentStatus;

/// How a provider signs callback notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookCapability {
    /// HMAC-SHA256 over a documented string (Stripe, Adyen, Square).
    HmacSha256,
    /// RSA (SHA-256) signature over ordered parameters (Alipay).
    Rsa2,
    /// RSA signature plus AEAD_AES_256_GCM encrypted resource (WeChat Pay v3).
    Rsa2Aes256Gcm,
    /// Signature verified by calling the provider's API (PayPal).
    ProviderApi,
    /// No callback signature scheme.
    None,
}

/// Currency restriction declared by an adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrencyScope {
    /// Any well-formed ISO 4217 currency.
    IsoAll,
    /// Only the listed codes (domestic CNY-only channels, for example).
    Only(&'static [&'static str]),
}

impl CurrencyScope {
    /// Whether `currency` is accepted.
    pub fn supports(&self, currency: &Currency) -> bool {
        match self {
            Self::IsoAll => true,
            Self::Only(codes) => codes
                .iter()
                .any(|code| code.eq_ignore_ascii_case(currency.code())),
        }
    }
}

/// What one adapter implements.
///
/// Capability flags are advisory but authoritative: callers can pre-check and
/// adapters return [`PaymentError::Unsupported`](crate::PaymentError::Unsupported)
/// when a disabled operation is invoked anyway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCapabilities {
    /// Create a payment intent/order.
    pub create: bool,
    /// Query a payment.
    pub query: bool,
    /// Capture an authorized payment.
    pub capture: bool,
    /// Cancel/close/void a payment.
    pub cancel: bool,
    /// Refund (full or partial).
    pub refund: bool,
    /// Partial refunds are supported.
    pub partial_refund: bool,
    /// Provider-hosted checkout page / payment link.
    pub hosted_checkout: bool,
    /// QR-code channel.
    pub qr: bool,
    /// App-SDK channel.
    pub app: bool,
    /// Callback signature scheme.
    pub webhook: WebhookCapability,
    /// Currencies accepted by the adapter.
    pub currencies: CurrencyScope,
    /// Statuses the adapter can report (documentation/telemetry aid).
    pub statuses: &'static [PaymentStatus],
}

impl ProviderCapabilities {
    /// A capability set with everything disabled.
    pub const fn none() -> Self {
        Self {
            create: false,
            query: false,
            capture: false,
            cancel: false,
            refund: false,
            partial_refund: false,
            hosted_checkout: false,
            qr: false,
            app: false,
            webhook: WebhookCapability::None,
            currencies: CurrencyScope::IsoAll,
            statuses: &[],
        }
    }

    /// The common payment shape: create/query/cancel/refund, no capture.
    pub const fn basic(webhook: WebhookCapability) -> Self {
        Self {
            create: true,
            query: true,
            capture: false,
            cancel: true,
            refund: true,
            partial_refund: true,
            hosted_checkout: false,
            qr: false,
            app: false,
            webhook,
            currencies: CurrencyScope::IsoAll,
            statuses: &[],
        }
    }

    /// Whether `currency` is accepted.
    pub fn supports_currency(&self, currency: &Currency) -> bool {
        self.currencies.supports(currency)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn currency_scope_only_lists_exact_codes() {
        let scope = CurrencyScope::Only(&["CNY", "HKD"]);
        assert!(scope.supports(&Currency::CNY));
        assert!(scope.supports(&Currency::HKD));
        assert!(!scope.supports(&Currency::USD));
        assert!(CurrencyScope::IsoAll.supports(&Currency::USD));
    }
}
