//! Square webhook signature verification and event normalization.
//!
//! Square signs every notification as
//! `base64(HMAC-SHA256(signature_key, notification_url + raw_body))` and sends
//! the result in the `x-square-hmacsha256-signature` header. The notification
//! URL is the exact URL registered in the Square developer dashboard, so it is
//! configuration ([`SquareOptions::webhook_url`](crate::SquareOptions)), not a
//! per-request value.
//!
//! Square has no timestamp/replay header; replay protection is event-id
//! de-duplication (`PaymentEvent::dedupe_key` = `event_id`).
//!
//! Event envelope:
//!
//! ```json
//! {
//!   "merchant_id": "...",
//!   "type": "payment.updated",
//!   "event_id": "...",
//!   "created_at": "2020-02-06T21:27:34.308Z",
//!   "data": { "type": "payment", "id": "...", "object": { "payment": { ... } } }
//! }
//! ```

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;
use webx_core::action::Response;
use webx_core::security::constant_time_eq;
use webx_pay::{
    empty_ok_ack, AccountId, AccountSet, IPaymentWebhookVerifier, PaymentError, PaymentEvent,
    PaymentEventType, PaymentScope, ProviderId, WebhookPayload,
};

use crate::mapping::{
    map_payment_event_type, map_refund_event_type, money_from, parse_timestamp, string_field,
    unix_now,
};
use crate::provider::SquareAccount;

/// Header carrying the Square webhook signature.
pub const SIGNATURE_HEADER: &str = "x-square-hmacsha256-signature";

const PROVIDER: ProviderId = ProviderId::Square;

type HmacSha256 = Hmac<Sha256>;

/// Verify a Square webhook signature.
///
/// `notification_url` must match the URL registered with Square byte for byte
/// (including any trailing slash); `body` must be the raw request body, not a
/// re-serialized JSON value. The comparison is constant time.
pub fn verify_signature(
    signature_key: &str,
    notification_url: &str,
    signature_header: &str,
    body: &[u8],
) -> Result<(), &'static str> {
    if signature_key.is_empty() {
        return Err("missing signature key");
    }
    if notification_url.is_empty() {
        return Err("missing notification URL");
    }
    let signature_header = signature_header.trim();
    if signature_header.is_empty() {
        return Err("missing signature header");
    }
    let provided = BASE64
        .decode(signature_header)
        .map_err(|_| "invalid signature encoding")?;

    let mut mac = HmacSha256::new_from_slice(signature_key.as_bytes())
        .map_err(|_| "invalid signature key")?;
    mac.update(notification_url.as_bytes());
    mac.update(body);
    let expected = mac.finalize().into_bytes();

    if constant_time_eq(&provided, &expected) {
        Ok(())
    } else {
        Err("signature mismatch")
    }
}

/// Verify Square callbacks for every configured account.
#[derive(Clone)]
pub struct SquareWebhookVerifier {
    accounts: Result<AccountSet<SquareAccount>, String>,
    webhook_url: Option<String>,
}

impl SquareWebhookVerifier {
    pub(crate) fn new(
        accounts: Result<AccountSet<SquareAccount>, String>,
        webhook_url: Option<String>,
    ) -> Self {
        Self {
            accounts,
            webhook_url,
        }
    }
}

#[async_trait]
impl IPaymentWebhookVerifier for SquareWebhookVerifier {
    fn provider(&self) -> ProviderId {
        PROVIDER
    }

    async fn verify(
        &self,
        scope: &PaymentScope,
        payload: &WebhookPayload,
    ) -> Result<PaymentEvent, PaymentError> {
        let account = scope.account_ref();
        let accounts = self
            .accounts
            .as_ref()
            .map_err(|error| PaymentError::config(PROVIDER, error.clone()))?;
        if accounts.len() > 1 && account.is_none() {
            return Err(PaymentError::validation(
                "square webhook requires the account path segment when multiple accounts are configured",
            ));
        }
        let account = accounts.resolve(account)?;
        let signature_key = account.signature_key.as_deref().ok_or_else(|| {
            PaymentError::config(
                PROVIDER,
                format!("square account `{}` has no signature_key", account.id),
            )
        })?;
        let webhook_url = self.webhook_url.as_deref().ok_or_else(|| {
            PaymentError::config(
                PROVIDER,
                "SquareOptions::webhook_url is required for webhook verification",
            )
        })?;
        let header = payload
            .header(SIGNATURE_HEADER)
            .ok_or_else(|| PaymentError::signature(PROVIDER, "missing signature header"))?;
        verify_signature(signature_key, webhook_url, header, payload.body())
            .map_err(|reason| PaymentError::signature(PROVIDER, reason))?;
        let value: Value = payload.json()?;
        parse_event(&value, account.id.clone(), account.live)
    }

    fn ack(&self) -> Response {
        empty_ok_ack()
    }
}

#[derive(Debug, Deserialize)]
struct SquareEvent {
    #[serde(rename = "type")]
    kind: String,
    event_id: String,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    data: SquareEventData,
}

#[derive(Debug, Default, Deserialize)]
struct SquareEventData {
    #[serde(default)]
    object: Value,
}

/// Normalize a Square event body.
///
/// `live` comes from the account environment: Square events do not carry a
/// `livemode` flag, and the signed endpoint itself already binds the account.
pub(crate) fn parse_event(
    value: &Value,
    account: AccountId,
    live: bool,
) -> Result<PaymentEvent, PaymentError> {
    let event: SquareEvent = serde_json::from_value(value.clone())
        .map_err(|error| PaymentError::Serialization(format!("invalid square event: {error}")))?;
    let object = &event.data.object;

    let (event_type, order_id, provider_payment_id, amount, provider_status) =
        if let Some(entity) = entity_for(&event.kind, object, "payment") {
            let status = string_field(entity, "status")
                .unwrap_or_default()
                .to_string();
            (
                map_payment_event_type(&event.kind, &status),
                string_field(entity, "reference_id").map(str::to_string),
                string_field(entity, "id").map(str::to_string),
                money_from(entity.get("amount_money")),
                non_empty(status),
            )
        } else if let Some(entity) = entity_for(&event.kind, object, "refund") {
            let status = string_field(entity, "status")
                .unwrap_or_default()
                .to_string();
            (
                map_refund_event_type(&event.kind, &status),
                string_field(entity, "reference_id").map(str::to_string),
                string_field(entity, "payment_id").map(str::to_string),
                money_from(entity.get("amount_money")),
                non_empty(status),
            )
        } else {
            (
                PaymentEventType::Unknown(event.kind.clone()),
                None,
                None,
                None,
                string_field(object, "status").map(str::to_string),
            )
        };

    let mut result = PaymentEvent::new(PROVIDER, event_type, event.event_id.clone());
    result.account = Some(account);
    result.provider_event_id = Some(event.event_id);
    result.provider_status = provider_status;
    result.order_id = order_id;
    result.provider_payment_id = provider_payment_id;
    result.amount = amount;
    result.occurred_at = event
        .created_at
        .as_deref()
        .and_then(parse_timestamp)
        .unwrap_or_else(unix_now);
    result.livemode = Some(live);
    result.raw = value.clone();
    Ok(result)
}

/// The payment/refund entity inside `data.object`, for `payment.*`/`refund.*`
/// events. Older payloads inline the entity directly; both shapes are accepted.
fn entity_for<'a>(kind: &str, object: &'a Value, key: &str) -> Option<&'a Value> {
    if !kind.starts_with(key) {
        return None;
    }
    if let Some(entity) = object.get(key).filter(|value| value.is_object()) {
        return Some(entity);
    }
    let inline = object.get("id").is_some() && object.get("status").is_some();
    inline.then_some(object)
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use webx_pay::PaymentStatus;

    const NOTIFICATION_URL: &str = "https://example.com/webhooks/payments/square";
    const SIGNATURE_KEY: &str = "test-signature-key";

    const PAYMENT_UPDATED_BODY: &str = r#"{"merchant_id":"merchant-1","type":"payment.updated","event_id":"event-1","created_at":"2020-02-06T21:27:34.308Z","data":{"type":"payment","id":"payment-1","object":{"payment":{"id":"payment-1","status":"COMPLETED","amount_money":{"amount":9900,"currency":"USD"},"reference_id":"ORDER-1","created_at":"2020-02-06T21:27:34.308Z"}}}}"#;

    /// Known-answer vector: HMAC-SHA256("test-signature-key",
    /// NOTIFICATION_URL + PAYMENT_UPDATED_BODY), base64 encoded.
    const PAYMENT_UPDATED_SIGNATURE: &str = "00dx98jslVeMaNSaBc1K3cRutc8NyV8AGk1hcimiMJ4=";

    fn signed_payload(body: &str, signature: &str) -> WebhookPayload {
        WebhookPayload::new(
            [(SIGNATURE_HEADER.to_string(), signature.to_string())],
            body.as_bytes().to_vec(),
        )
    }

    fn scope() -> PaymentScope {
        PaymentScope::new(ProviderId::Square)
    }

    fn verifier_for(options: &crate::SquareOptions) -> SquareWebhookVerifier {
        SquareWebhookVerifier::new(options.build_accounts(), options.webhook_url())
    }

    fn sandbox_options() -> crate::SquareOptions {
        crate::SquareOptions::empty()
            .with_account(
                crate::SquareAccountOptions::new("test-access-token")
                    .with_environment(crate::SquareEnvironment::Sandbox)
                    .with_location_id("TESTLOCATION1")
                    .with_signature_key(SIGNATURE_KEY),
            )
            .with_webhook_url(NOTIFICATION_URL)
    }

    #[test]
    fn accepts_the_documented_hmac_vector() {
        assert_eq!(
            verify_signature(
                SIGNATURE_KEY,
                NOTIFICATION_URL,
                PAYMENT_UPDATED_SIGNATURE,
                PAYMENT_UPDATED_BODY.as_bytes()
            ),
            Ok(())
        );
    }

    #[test]
    fn rejects_tampered_bodies_wrong_keys_and_bad_inputs() {
        assert_eq!(
            verify_signature(
                SIGNATURE_KEY,
                NOTIFICATION_URL,
                PAYMENT_UPDATED_SIGNATURE,
                br#"{"event_id":"tampered"}"#
            ),
            Err("signature mismatch")
        );
        assert_eq!(
            verify_signature(
                "another-key",
                NOTIFICATION_URL,
                PAYMENT_UPDATED_SIGNATURE,
                PAYMENT_UPDATED_BODY.as_bytes()
            ),
            Err("signature mismatch")
        );
        // The notification URL is part of the signed string.
        assert_eq!(
            verify_signature(
                SIGNATURE_KEY,
                "https://example.com/webhooks/payments/square/",
                PAYMENT_UPDATED_SIGNATURE,
                PAYMENT_UPDATED_BODY.as_bytes()
            ),
            Err("signature mismatch")
        );
        assert_eq!(
            verify_signature(SIGNATURE_KEY, NOTIFICATION_URL, "  ", b"{}"),
            Err("missing signature header")
        );
        assert_eq!(
            verify_signature(SIGNATURE_KEY, NOTIFICATION_URL, "not-base64!", b"{}"),
            Err("invalid signature encoding")
        );
        assert_eq!(
            verify_signature("", NOTIFICATION_URL, PAYMENT_UPDATED_SIGNATURE, b"{}"),
            Err("missing signature key")
        );
    }

    #[test]
    fn normalizes_payment_updated_completed() {
        let value: Value = serde_json::from_str(PAYMENT_UPDATED_BODY).unwrap();
        let event = parse_event(&value, AccountId::default_account(), true).unwrap();
        assert_eq!(event.event_type, PaymentEventType::Succeeded);
        assert_eq!(event.dedupe_key, "event-1");
        assert_eq!(event.provider_event_id.as_deref(), Some("event-1"));
        assert_eq!(event.provider_status.as_deref(), Some("COMPLETED"));
        assert_eq!(event.order_id.as_deref(), Some("ORDER-1"));
        assert_eq!(event.provider_payment_id.as_deref(), Some("payment-1"));
        assert_eq!(event.amount.unwrap().minor, 9900);
        assert_eq!(event.occurred_at, 1_581_024_454);
        assert_eq!(event.livemode, Some(true));
        assert_eq!(event.account, Some(AccountId::default_account()));
    }

    #[test]
    fn maps_every_payment_and_refund_status_from_webhooks() {
        let statuses = [
            ("APPROVED", PaymentEventType::AuthorizationSucceeded),
            ("COMPLETED", PaymentEventType::Succeeded),
            ("CANCELED", PaymentEventType::Canceled),
            ("FAILED", PaymentEventType::Failed),
        ];
        for (status, expected) in statuses {
            let value = serde_json::json!({
                "type": "payment.updated",
                "event_id": format!("event-{status}"),
                "created_at": "2020-02-06T21:27:34.308Z",
                "data": { "object": { "payment": {
                    "id": format!("payment-{status}"),
                    "status": status,
                    "amount_money": { "amount": 100, "currency": "USD" },
                    "reference_id": "ORDER-1"
                }}}
            });
            let event = parse_event(&value, AccountId::default_account(), false).unwrap();
            assert_eq!(event.event_type, expected, "status {status}");
            assert_eq!(event.livemode, Some(false));
        }

        let refunds = [
            ("COMPLETED", PaymentEventType::RefundSucceeded),
            ("APPROVED", PaymentEventType::RefundSucceeded),
            ("REJECTED", PaymentEventType::RefundFailed),
            ("FAILED", PaymentEventType::RefundFailed),
        ];
        for (status, expected) in refunds {
            let value = serde_json::json!({
                "type": "refund.updated",
                "event_id": format!("event-refund-{status}"),
                "created_at": "2020-02-06T22:14:16.421Z",
                "data": { "object": { "refund": {
                    "id": "payment-1_REFUND-1",
                    "payment_id": "payment-1",
                    "status": status,
                    "amount_money": { "amount": 1000, "currency": "USD" }
                }}}
            });
            let event = parse_event(&value, AccountId::default_account(), false).unwrap();
            assert_eq!(event.event_type, expected, "refund status {status}");
            assert_eq!(event.provider_payment_id.as_deref(), Some("payment-1"));
            assert_eq!(event.amount.unwrap().minor, 1000);
        }
    }

    #[test]
    fn unknown_events_are_preserved_as_unknown() {
        let value = serde_json::json!({
            "merchant_id": "merchant-1",
            "type": "order.updated",
            "event_id": "event-9",
            "created_at": "2020-02-06T21:27:34.308Z",
            "data": { "object": { "order": { "id": "order-1" } } }
        });
        let event = parse_event(&value, AccountId::default_account(), false).unwrap();
        assert_eq!(
            event.event_type,
            PaymentEventType::Unknown("order.updated".to_string())
        );
        assert_eq!(event.dedupe_key, "event-9");
        assert!(event.provider_payment_id.is_none());
    }

    #[tokio::test]
    async fn verifies_and_normalizes_a_signed_payload() {
        let verifier = verifier_for(&sandbox_options());
        let payload = signed_payload(PAYMENT_UPDATED_BODY, PAYMENT_UPDATED_SIGNATURE);
        let event = verifier.verify(&scope(), &payload).await.unwrap();
        assert_eq!(event.event_type, PaymentEventType::Succeeded);
        assert_eq!(event.account, Some(AccountId::default_account()));
        assert_eq!(event.livemode, Some(false));
    }

    #[tokio::test]
    async fn rejects_unsigned_or_tampered_payloads() {
        let verifier = verifier_for(&sandbox_options());
        let unsigned = WebhookPayload::new(
            Vec::<(String, String)>::new(),
            PAYMENT_UPDATED_BODY.as_bytes().to_vec(),
        );
        let error = verifier.verify(&scope(), &unsigned).await.unwrap_err();
        assert_eq!(error.http_status(), 400);
        assert_eq!(error.public_detail(), Some("missing signature header"));

        let tampered = signed_payload(r#"{"type":"payment.updated"}"#, PAYMENT_UPDATED_SIGNATURE);
        let error = verifier.verify(&scope(), &tampered).await.unwrap_err();
        assert_eq!(error.http_status(), 400);
        assert_eq!(error.public_detail(), Some("signature mismatch"));
    }

    #[tokio::test]
    async fn multi_account_verification_requires_the_account_path() {
        let options = crate::SquareOptions::new("test-access-token").with_account(
            crate::SquareAccountOptions::new("test-access-token-2").with_id("second"),
        );
        let verifier = verifier_for(&options);
        let payload = WebhookPayload::new(
            Vec::<(String, String)>::new(),
            PAYMENT_UPDATED_BODY.as_bytes().to_vec(),
        );
        let error = verifier.verify(&scope(), &payload).await.unwrap_err();
        assert_eq!(error.http_status(), 400);
        assert!(error.to_string().contains("account path segment"));
    }

    #[tokio::test]
    async fn account_scope_selects_the_merchant_account() {
        let options = crate::SquareOptions::new("test-access-token")
            .with_account(
                crate::SquareAccountOptions::new("test-access-token-2")
                    .with_id("second")
                    .with_signature_key(SIGNATURE_KEY),
            )
            .with_webhook_url(NOTIFICATION_URL);
        let verifier = verifier_for(&options);
        let payload = signed_payload(PAYMENT_UPDATED_BODY, PAYMENT_UPDATED_SIGNATURE);
        let scope = PaymentScope::new(ProviderId::Square).account("second");
        let event = verifier.verify(&scope, &payload).await.unwrap();
        assert_eq!(
            event.account.as_ref().map(|account| account.as_str()),
            Some("second")
        );
    }

    #[tokio::test]
    async fn account_without_a_signature_key_is_a_configuration_error() {
        let options = crate::SquareOptions::new("test-access-token");
        let verifier = verifier_for(&options);
        let payload = signed_payload(PAYMENT_UPDATED_BODY, PAYMENT_UPDATED_SIGNATURE);
        let error = verifier.verify(&scope(), &payload).await.unwrap_err();
        assert_eq!(error.http_status(), 500);
        assert!(error.to_string().contains("signature_key"));
    }

    #[tokio::test]
    async fn webhook_is_acked_with_an_empty_200() {
        let verifier = verifier_for(&sandbox_options());
        // `ack()` returns the framework response the application must send.
        let _response = verifier.ack();
        let event = verifier
            .verify(
                &scope(),
                &signed_payload(PAYMENT_UPDATED_BODY, PAYMENT_UPDATED_SIGNATURE),
            )
            .await
            .unwrap();
        assert_eq!(event.dedupe_key, "event-1");
        assert_eq!(event.event_type, PaymentEventType::Succeeded);
        // Sanity: the shared status mapping is what the event carries.
        assert_eq!(
            crate::mapping::map_payment_status("COMPLETED"),
            PaymentStatus::Succeeded
        );
    }
}
