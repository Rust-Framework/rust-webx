//! Stripe webhook signature verification and event normalization.

use async_trait::async_trait;
use data_encoding::HEXLOWER;
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use webx_core::action::Response;
use webx_core::security::constant_time_eq;
use webx_pay::{
    empty_ok_ack, AccountId, AccountSet, IPaymentWebhookVerifier, PaymentError, PaymentEvent,
    PaymentEventType, PaymentScope, ProviderId, WebhookPayload,
};

use crate::form::unix_now;
use crate::mapping::{int_field, money_from, order_id_from, string_field};

/// Stripe's documented default timestamp tolerance (5 minutes).
pub const SIGNATURE_TOLERANCE_SECONDS: i64 = 300;

type HmacSha256 = Hmac<Sha256>;

/// Verify a `Stripe-Signature` header against `body`.
///
/// Header format: `t=<unix>,v1=<hex>[,v1=<hex>...]` plus optional debug entries
/// (`v0`, `secret`, `destination`, `payload`) that must be ignored. Multiple
/// `v1` values are accepted during endpoint-secret rotation.
pub fn verify_signature(
    signing_material: &str,
    header: &str,
    body: &[u8],
    now: i64,
) -> Result<(), &'static str> {
    if header.trim().is_empty() {
        return Err("missing signature header");
    }
    let mut timestamp: Option<i64> = None;
    let mut signatures: Vec<&str> = Vec::new();
    for part in header.split(',') {
        let Some((key, value)) = part.trim().split_once('=') else {
            continue;
        };
        match key {
            "t" => {
                timestamp = value.parse::<i64>().ok();
            }
            "v1" => signatures.push(value),
            // v0, secret, destination, payload are debug/legacy metadata.
            _ => {}
        }
    }
    let timestamp = timestamp.ok_or("missing signature timestamp")?;
    if (now - timestamp).abs() > SIGNATURE_TOLERANCE_SECONDS {
        return Err("timestamp outside tolerance");
    }
    if signatures.is_empty() {
        return Err("no v1 signature");
    }

    let mut mac = HmacSha256::new_from_slice(signing_material.as_bytes())
        .map_err(|_| "invalid webhook secret")?;
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    let expected_hex = HEXLOWER.encode(&mac.finalize().into_bytes());

    let matched = signatures.iter().any(|candidate| {
        let candidate = candidate.trim().to_ascii_lowercase();
        constant_time_eq(candidate.as_bytes(), expected_hex.as_bytes())
    });
    if matched {
        Ok(())
    } else {
        Err("signature mismatch")
    }
}

#[derive(Debug, serde::Deserialize)]
struct StripeEvent {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    created: Option<i64>,
    #[serde(default)]
    livemode: Option<bool>,
    #[serde(default)]
    data: StripeEventData,
}

#[derive(Debug, Default, serde::Deserialize)]
struct StripeEventData {
    #[serde(default)]
    object: Value,
}

/// Verify callbacks for every configured Stripe account.
#[derive(Clone)]
pub struct StripeWebhookVerifier {
    accounts: Result<AccountSet<crate::provider::StripeAccount>, String>,
}

impl StripeWebhookVerifier {
    pub(crate) fn new(
        accounts: Result<AccountSet<crate::provider::StripeAccount>, String>,
    ) -> Self {
        Self { accounts }
    }
}

#[async_trait]
impl IPaymentWebhookVerifier for StripeWebhookVerifier {
    fn provider(&self) -> ProviderId {
        ProviderId::Stripe
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
            .map_err(|error| PaymentError::config(ProviderId::Stripe, error.clone()))?;
        if accounts.len() > 1 && account.is_none() {
            return Err(PaymentError::validation(
                "stripe webhook requires the account path segment when multiple accounts are configured",
            ));
        }
        let account = accounts.resolve(account)?;
        let header = payload.header("stripe-signature").ok_or_else(|| {
            PaymentError::signature(ProviderId::Stripe, "missing signature header")
        })?;
        let signing_material = account.webhook_secret.as_deref().ok_or_else(|| {
            PaymentError::config(
                ProviderId::Stripe,
                format!("stripe account `{}` has no webhook secret", account.id),
            )
        })?;
        verify_signature(signing_material, header, payload.body(), unix_now())
            .map_err(|reason| PaymentError::signature(ProviderId::Stripe, reason))?;
        let value: Value = payload.json()?;
        parse_event(&value, account.id.clone())
    }

    fn ack(&self) -> Response {
        empty_ok_ack()
    }
}

/// Normalize a Stripe event body.
pub fn parse_event(value: &Value, account: AccountId) -> Result<PaymentEvent, PaymentError> {
    let event: StripeEvent = serde_json::from_value(value.clone())
        .map_err(|error| PaymentError::Serialization(format!("invalid stripe event: {error}")))?;
    let object = &event.data.object;

    let event_type = match event.kind.as_str() {
        "payment_intent.succeeded" => PaymentEventType::Succeeded,
        "payment_intent.amount_capturable_updated" => PaymentEventType::AuthorizationSucceeded,
        "payment_intent.payment_failed" => PaymentEventType::Failed,
        "payment_intent.canceled" => PaymentEventType::Canceled,
        "checkout.session.completed" => {
            match string_field(object, "payment_status").unwrap_or_default() {
                "paid" | "no_payment_required" => PaymentEventType::Succeeded,
                _ => PaymentEventType::Unknown(event.kind.clone()),
            }
        }
        "checkout.session.async_payment_succeeded" => PaymentEventType::Succeeded,
        "checkout.session.async_payment_failed" => PaymentEventType::Failed,
        "checkout.session.expired" => PaymentEventType::Canceled,
        "charge.refunded" | "charge.refund.updated" | "refund.updated" => {
            match string_field(object, "status").unwrap_or_default() {
                "failed" | "canceled" => PaymentEventType::RefundFailed,
                _ => PaymentEventType::RefundSucceeded,
            }
        }
        "charge.dispute.created" => PaymentEventType::DisputeOpened,
        other => PaymentEventType::Unknown(other.to_string()),
    };

    let amount = money_from(
        int_field(object, "amount_received")
            .or_else(|| int_field(object, "amount"))
            .or_else(|| int_field(object, "amount_total"))
            .or_else(|| int_field(object, "amount_refunded")),
        string_field(object, "currency"),
    );

    let provider_payment_id = if event.kind.starts_with("checkout.session") {
        string_field(object, "payment_intent")
            .map(str::to_string)
            .or_else(|| Some(event.id.clone()))
    } else {
        string_field(object, "id")
            .map(str::to_string)
            .or_else(|| string_field(object, "payment_intent").map(str::to_string))
    };

    let provider_status = string_field(object, "status")
        .or_else(|| string_field(object, "payment_status"))
        .unwrap_or_default()
        .to_string();

    let mut result = PaymentEvent::new(ProviderId::Stripe, event_type, event.id.clone());
    result.account = Some(account);
    result.provider_event_id = Some(event.id);
    result.order_id = order_id_from(object);
    result.provider_payment_id = provider_payment_id;
    result.amount = amount;
    result.occurred_at = event.created.unwrap_or_else(unix_now);
    result.livemode = event.livemode;
    result.raw = value.clone();
    result.provider_status = (!provider_status.is_empty()).then_some(provider_status);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_header(signing_material: &str, timestamp: i64, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(signing_material.as_bytes()).unwrap();
        mac.update(timestamp.to_string().as_bytes());
        mac.update(b".");
        mac.update(body);
        let hex = HEXLOWER.encode(&mac.finalize().into_bytes());
        format!("t={timestamp},v1={hex}")
    }

    #[test]
    fn accepts_a_valid_signature_and_ignores_unknown_entries() {
        let signing_material = "fixture-value-1";
        let body = br#"{"id":"evt_1"}"#;
        let header = format!(
            "{},v0=deadbeef,secret=ignored,destination=we_1,payload=abc",
            signed_header(signing_material, 1_700_000_000, body)
        );
        assert!(verify_signature(signing_material, &header, body, 1_700_000_010).is_ok());
    }

    #[test]
    fn rejects_tampered_body_and_stale_timestamp() {
        let signing_material = "fixture-value-1";
        let body = br#"{"id":"evt_1"}"#;
        let header = signed_header(signing_material, 1_700_000_000, body);
        assert!(verify_signature(
            signing_material,
            &header,
            br#"{"id":"evt_2"}"#,
            1_700_000_000
        )
        .is_err());
        assert_eq!(
            verify_signature(signing_material, &header, body, 1_700_000_301),
            Err("timestamp outside tolerance")
        );
    }

    #[test]
    fn accepts_any_v1_value_during_secret_rotation() {
        let signing_material = "fixture-value-2";
        let body = b"{}";
        let valid = signed_header(signing_material, 1_700_000_000, body);
        let v1 = valid.split(',').nth(1).unwrap();
        let header = format!("t=1700000000,v1=deadbeef,{v1}");
        assert!(verify_signature(signing_material, &header, body, 1_700_000_000).is_ok());
    }

    #[test]
    fn normalizes_payment_intent_succeeded() {
        let value = serde_json::json!({
            "id": "evt_1",
            "type": "payment_intent.succeeded",
            "created": 1700000000,
            "livemode": false,
            "data": { "object": {
                "id": "pi_1",
                "status": "succeeded",
                "amount": 9900,
                "amount_received": 9900,
                "currency": "cny",
                "metadata": { "order_id": "ORDER-1" }
            }}
        });
        let event = parse_event(&value, AccountId::default_account()).unwrap();
        assert_eq!(event.event_type, PaymentEventType::Succeeded);
        assert_eq!(event.order_id.as_deref(), Some("ORDER-1"));
        assert_eq!(event.provider_payment_id.as_deref(), Some("pi_1"));
        assert_eq!(event.amount.unwrap().minor, 9900);
        assert_eq!(event.dedupe_key, "evt_1");
    }

    #[test]
    fn normalizes_checkout_session_completion() {
        let value = serde_json::json!({
            "id": "evt_2",
            "type": "checkout.session.completed",
            "created": 1700000000,
            "data": { "object": {
                "id": "cs_1",
                "status": "complete",
                "payment_status": "paid",
                "payment_intent": "pi_2",
                "amount_total": 500,
                "currency": "usd",
                "client_reference_id": "ORDER-2"
            }}
        });
        let event = parse_event(&value, AccountId::default_account()).unwrap();
        assert_eq!(event.event_type, PaymentEventType::Succeeded);
        assert_eq!(event.order_id.as_deref(), Some("ORDER-2"));
        assert_eq!(event.provider_payment_id.as_deref(), Some("pi_2"));
    }
}
