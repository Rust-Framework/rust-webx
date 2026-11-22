//! Provider and webhook-verifier ports.

use bytes::Bytes;
use serde_json::Value;
use webx_core::action::Response;

use crate::capabilities::ProviderCapabilities;
use crate::error::PaymentError;
use crate::events::PaymentEvent;
use crate::ids::ProviderId;
use crate::model::{CreatePayment, PaymentAccount, PaymentIntent, Refund, RefundCommand};
use crate::money::Money;
use crate::scope::PaymentScope;

/// A raw callback request: headers plus the exact body bytes.
///
/// Signature schemes are defined over the raw body, so adapters never receive a
/// parsed JSON value. Header lookup is case-insensitive.
#[derive(Debug, Clone)]
pub struct WebhookPayload {
    headers: Vec<(String, String)>,
    body: Bytes,
}

impl WebhookPayload {
    /// Build a payload from header pairs and raw body bytes.
    pub fn new(
        headers: impl IntoIterator<Item = (String, String)>,
        body: impl Into<Bytes>,
    ) -> Self {
        Self {
            headers: headers.into_iter().collect(),
            body: body.into(),
        }
    }

    /// First header value with `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// All header pairs, in wire order.
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// Raw body bytes.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Shared body handle.
    pub fn body_bytes(&self) -> &Bytes {
        &self.body
    }

    /// Parse the body as JSON (callers should verify the signature first).
    pub fn json(&self) -> Result<Value, PaymentError> {
        serde_json::from_slice(&self.body)
            .map_err(|error| PaymentError::Serialization(format!("invalid callback JSON: {error}")))
    }

    /// Parse the body as a form-encoded map (Alipay notifications).
    pub fn form(&self) -> Result<Vec<(String, String)>, PaymentError> {
        let text = std::str::from_utf8(&self.body).map_err(|error| {
            PaymentError::Serialization(format!("callback body is not UTF-8: {error}"))
        })?;
        Ok(url::form_urlencoded::parse(text.as_bytes())
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect())
    }
}

/// A payment provider adapter.
///
/// One implementation serves one [`ProviderId`] and can hold several merchant
/// accounts; the operation receives a [`PaymentScope`] and selects the account
/// from `scope.account_ref()`. Unsupported operations return
/// [`PaymentError::Unsupported`] through the default methods.
#[async_trait::async_trait]
pub trait IPaymentProvider: Send + Sync {
    /// Provider id owned by this adapter.
    fn id(&self) -> ProviderId;

    /// Public account views (never credentials).
    fn accounts(&self) -> &[PaymentAccount];

    /// Declared capabilities.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Public frontend catalog for this provider's configured accounts.
    ///
    /// Must never contain secret credentials. The default is empty so a custom
    /// adapter can opt in incrementally.
    fn payment_options(&self) -> Vec<crate::PaymentOption> {
        Vec::new()
    }

    /// Static validation: required fields, parseable keys/certificates,
    /// HTTPS endpoints, supported currencies. Must not perform network calls.
    fn validate(&self) -> Result<(), PaymentError>;

    /// Create a payment (or a hosted checkout intent).
    async fn create_payment(
        &self,
        scope: &PaymentScope,
        request: CreatePayment,
    ) -> Result<PaymentIntent, PaymentError>;

    /// Query a payment by merchant order id.
    async fn get_payment(
        &self,
        scope: &PaymentScope,
        order_id: &str,
    ) -> Result<PaymentIntent, PaymentError>;

    /// Capture an authorized payment (manual-capture providers only).
    async fn capture_payment(
        &self,
        _scope: &PaymentScope,
        _order_id: &str,
        _amount: Option<Money>,
    ) -> Result<PaymentIntent, PaymentError> {
        Err(PaymentError::unsupported(self.id(), "capture"))
    }

    /// Cancel/close/void a payment.
    async fn cancel_payment(
        &self,
        _scope: &PaymentScope,
        _order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        Err(PaymentError::unsupported(self.id(), "cancel"))
    }

    /// Refund a payment (full or partial).
    async fn refund(
        &self,
        _scope: &PaymentScope,
        _request: RefundCommand,
    ) -> Result<Refund, PaymentError> {
        Err(PaymentError::unsupported(self.id(), "refund"))
    }

    /// Query a refund by merchant refund id.
    async fn get_refund(
        &self,
        _scope: &PaymentScope,
        _refund_id: &str,
    ) -> Result<Refund, PaymentError> {
        Err(PaymentError::unsupported(self.id(), "get_refund"))
    }

    /// Callback verifier for this provider.
    fn webhook_verifier(&self) -> &dyn IPaymentWebhookVerifier;
}

/// Verifies callback authenticity and normalizes the event.
///
/// Verification can be asynchronous because PayPal validates signatures by
/// calling its API.
#[async_trait::async_trait]
pub trait IPaymentWebhookVerifier: Send + Sync {
    /// Provider whose callbacks this verifier accepts.
    fn provider(&self) -> ProviderId;

    /// Verify `payload` for `scope` and return the normalized event.
    async fn verify(
        &self,
        scope: &PaymentScope,
        payload: &WebhookPayload,
    ) -> Result<PaymentEvent, PaymentError>;

    /// The success response the provider expects.
    fn ack(&self) -> Response;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_lookup_is_case_insensitive() {
        let payload = WebhookPayload::new(
            [("Stripe-Signature".to_string(), "t=1,v1=x".to_string())],
            b"{}".to_vec(),
        );
        assert_eq!(payload.header("stripe-signature"), Some("t=1,v1=x"));
        assert_eq!(payload.header("missing"), None);
    }

    #[test]
    fn form_parsing_decodes_percent_encoding() {
        let payload = WebhookPayload::new(
            [],
            b"out_trade_no=A%2FB&trade_status=TRADE_SUCCESS".to_vec(),
        );
        let form = payload.form().unwrap();
        assert!(form.contains(&("out_trade_no".to_string(), "A/B".to_string())));
    }
}
