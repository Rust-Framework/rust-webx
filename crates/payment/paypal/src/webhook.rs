//! PayPal Webhooks v1 verification and event normalization.
//!
//! PayPal signs callbacks with an asymmetric signature that is verified by
//! PayPal itself: the adapter forwards the `PAYPAL-TRANSMISSION-*`,
//! `PAYPAL-CERT-URL` and `PAYPAL-AUTH-ALGO` headers plus the raw event to
//! `POST /v1/notifications/verify-webhook-signature` and accepts only
//! `verification_status: SUCCESS`.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use webx_core::action::Response;
use webx_pay::{
    empty_ok_ack, AccountId, AccountSet, IPaymentWebhookVerifier, PaymentError, PaymentEvent,
    PaymentEventType, PaymentScope, ProviderId, WebhookPayload,
};

use crate::mapping::{money_from_resource, parse_rfc3339_secs, unix_now_secs};
use crate::options::PayPalAccount;
use crate::transport::{PayPalTransport, Verb};
use crate::PROVIDER;

/// Signature-verification headers sent with every PayPal webhook.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TransmissionHeaders<'a> {
    pub(crate) transmission_id: &'a str,
    pub(crate) transmission_time: &'a str,
    pub(crate) transmission_sig: &'a str,
    pub(crate) cert_url: &'a str,
    pub(crate) auth_algo: &'a str,
}

/// Extract the PayPal signature headers from a raw callback request.
pub(crate) fn extract_headers<'a>(
    payload: &'a WebhookPayload,
) -> Result<TransmissionHeaders<'a>, PaymentError> {
    let header = |name: &str, reason: &'static str| {
        payload
            .header(name)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| PaymentError::signature(PROVIDER, reason))
    };
    Ok(TransmissionHeaders {
        transmission_id: header("paypal-transmission-id", "missing PAYPAL-TRANSMISSION-ID")?,
        transmission_time: header(
            "paypal-transmission-time",
            "missing PAYPAL-TRANSMISSION-TIME",
        )?,
        transmission_sig: header("paypal-transmission-sig", "missing PAYPAL-TRANSMISSION-SIG")?,
        cert_url: header("paypal-cert-url", "missing PAYPAL-CERT-URL")?,
        auth_algo: header("paypal-auth-algo", "missing PAYPAL-AUTH-ALGO")?,
    })
}

/// Build the `POST /v1/notifications/verify-webhook-signature` request body.
pub(crate) fn build_verify_request(
    webhook_id: &str,
    headers: &TransmissionHeaders<'_>,
    event: &Value,
) -> Value {
    json!({
        "auth_algo": headers.auth_algo,
        "cert_url": headers.cert_url,
        "transmission_id": headers.transmission_id,
        "transmission_sig": headers.transmission_sig,
        "transmission_time": headers.transmission_time,
        "webhook_id": webhook_id,
        "webhook_event": event,
    })
}

#[derive(Debug, Default, Deserialize)]
struct WebhookEvent {
    #[serde(default)]
    id: String,
    #[serde(default)]
    event_type: String,
    #[serde(default)]
    create_time: Option<String>,
    #[serde(default)]
    resource: Value,
}

fn text_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

fn purchase_unit_order_id(resource: &Value) -> Option<&str> {
    let unit = resource.get("purchase_units")?.as_array()?.first()?;
    text_field(unit, "custom_id").or_else(|| text_field(unit, "reference_id"))
}

fn order_id_from_resource(resource: &Value) -> Option<String> {
    text_field(resource, "custom_id")
        .or_else(|| purchase_unit_order_id(resource))
        .map(str::to_string)
}

fn provider_status_from_resource(resource: &Value) -> Option<String> {
    text_field(resource, "status")
        .or_else(|| text_field(resource, "state"))
        .map(str::to_string)
}

/// Map a PayPal webhook event name to the shared event type.
pub(crate) fn map_event_type(event_type: &str) -> PaymentEventType {
    match event_type {
        "PAYMENT.CAPTURE.COMPLETED" => PaymentEventType::Succeeded,
        // Payments v2 uses DECLINED; v1 (still emitted for legacy webhooks) uses
        // DENIED. Both mean the capture will not settle.
        "PAYMENT.CAPTURE.DECLINED" | "PAYMENT.CAPTURE.DENIED" => PaymentEventType::Failed,
        "PAYMENT.CAPTURE.REFUNDED" | "PAYMENT.SALE.REFUNDED" => PaymentEventType::RefundSucceeded,
        "CHECKOUT.ORDER.APPROVED" => PaymentEventType::AuthorizationSucceeded,
        other => PaymentEventType::Unknown(other.to_string()),
    }
}

/// Normalize a PayPal webhook event body.
///
/// `dedupe_key` is the PayPal event id (`WH-...`), which is stable across
/// resends; callers persist it to make callback handling idempotent.
pub fn parse_event(value: &Value, account: AccountId) -> Result<PaymentEvent, PaymentError> {
    let event: WebhookEvent = serde_json::from_value(value.clone()).map_err(|error| {
        PaymentError::Serialization(format!("invalid paypal webhook event: {error}"))
    })?;
    if event.id.trim().is_empty() {
        return Err(PaymentError::Serialization(
            "paypal webhook event has no id".to_string(),
        ));
    }
    let resource = &event.resource;
    let mut normalized = PaymentEvent::new(
        PROVIDER,
        map_event_type(&event.event_type),
        event.id.clone(),
    );
    normalized.account = Some(account);
    normalized.provider_event_id = Some(event.id);
    normalized.order_id = order_id_from_resource(resource);
    normalized.provider_payment_id = text_field(resource, "id").map(str::to_string);
    normalized.amount = money_from_resource(resource);
    normalized.occurred_at = event
        .create_time
        .as_deref()
        .and_then(parse_rfc3339_secs)
        .unwrap_or_else(unix_now_secs);
    normalized.provider_status = provider_status_from_resource(resource);
    normalized.raw = value.clone();
    Ok(normalized)
}

/// Verifies PayPal callbacks for every configured account.
pub struct PayPalWebhookVerifier {
    accounts: Result<AccountSet<PayPalAccount>, String>,
    transport: Arc<PayPalTransport>,
}

impl PayPalWebhookVerifier {
    pub(crate) fn new(
        accounts: Result<AccountSet<PayPalAccount>, String>,
        transport: Arc<PayPalTransport>,
    ) -> Self {
        Self {
            accounts,
            transport,
        }
    }

    fn account_set(&self) -> Result<&AccountSet<PayPalAccount>, PaymentError> {
        self.accounts
            .as_ref()
            .map_err(|error| PaymentError::config(PROVIDER, error.clone()))
    }
}

#[async_trait]
impl IPaymentWebhookVerifier for PayPalWebhookVerifier {
    fn provider(&self) -> ProviderId {
        PROVIDER
    }

    async fn verify(
        &self,
        scope: &PaymentScope,
        payload: &WebhookPayload,
    ) -> Result<PaymentEvent, PaymentError> {
        let accounts = self.account_set()?;
        let account = scope.account_ref();
        if accounts.len() > 1 && account.is_none() {
            return Err(PaymentError::validation(
                "paypal webhook requires the account path segment when multiple accounts are configured",
            ));
        }
        let account = accounts.resolve(account)?;
        let webhook_id = account.webhook_id.as_deref().ok_or_else(|| {
            PaymentError::config(
                PROVIDER,
                format!("paypal account `{}` has no webhook_id", account.id),
            )
        })?;
        let headers = extract_headers(payload)?;
        let event = payload.json()?;
        let request = build_verify_request(webhook_id, &headers, &event);
        let response = self
            .transport
            .send(
                account,
                Verb::Post,
                "/v1/notifications/verify-webhook-signature",
                Some(&request),
                None,
                false,
            )
            .await?;
        match response
            .value
            .get("verification_status")
            .and_then(Value::as_str)
        {
            Some("SUCCESS") => {}
            Some("FAILURE") => {
                return Err(PaymentError::signature(
                    PROVIDER,
                    "provider rejected the webhook signature",
                ))
            }
            _ => {
                return Err(PaymentError::signature(
                    PROVIDER,
                    "verification response is not SUCCESS",
                ))
            }
        }
        parse_event(&event, account.id.clone())
    }

    fn ack(&self) -> Response {
        empty_ok_ack()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn webhook_payload(headers: &[(&str, &str)], body: &[u8]) -> WebhookPayload {
        WebhookPayload::new(
            headers
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
            body.to_vec(),
        )
    }

    fn signed_payload() -> WebhookPayload {
        webhook_payload(
            &[
                (
                    "PAYPAL-TRANSMISSION-ID",
                    "69cd13f0-d67a-11e5-baa3-778b53f4ae55",
                ),
                ("PAYPAL-TRANSMISSION-TIME", "2016-02-18T20:01:35Z"),
                ("PAYPAL-TRANSMISSION-SIG", "signature-value"),
                ("PAYPAL-CERT-URL", "https://api.paypal.com/cert.pem"),
                ("PAYPAL-AUTH-ALGO", "SHA256withRSA"),
            ],
            b"{}",
        )
    }

    #[test]
    fn extracts_signature_headers_case_insensitively() {
        let payload = signed_payload();
        let headers = extract_headers(&payload).unwrap();
        assert_eq!(
            headers.transmission_id,
            "69cd13f0-d67a-11e5-baa3-778b53f4ae55"
        );
        assert_eq!(headers.transmission_time, "2016-02-18T20:01:35Z");
        assert_eq!(headers.transmission_sig, "signature-value");
        assert_eq!(headers.cert_url, "https://api.paypal.com/cert.pem");
        assert_eq!(headers.auth_algo, "SHA256withRSA");
    }

    #[test]
    fn rejects_missing_or_empty_signature_headers() {
        let cases = [
            ("paypal-transmission-id", "missing PAYPAL-TRANSMISSION-ID"),
            (
                "paypal-transmission-time",
                "missing PAYPAL-TRANSMISSION-TIME",
            ),
            ("paypal-transmission-sig", "missing PAYPAL-TRANSMISSION-SIG"),
            ("paypal-cert-url", "missing PAYPAL-CERT-URL"),
            ("paypal-auth-algo", "missing PAYPAL-AUTH-ALGO"),
        ];
        for (missing, reason) in cases {
            let payload = webhook_payload(
                &[
                    ("PAYPAL-TRANSMISSION-ID", "id-1"),
                    ("PAYPAL-TRANSMISSION-TIME", "2016-02-18T20:01:35Z"),
                    ("PAYPAL-TRANSMISSION-SIG", "sig"),
                    ("PAYPAL-CERT-URL", "https://api.paypal.com/cert.pem"),
                    ("PAYPAL-AUTH-ALGO", "SHA256withRSA"),
                ],
                b"{}",
            );
            let headers: Vec<(String, String)> = payload
                .headers()
                .iter()
                .filter(|(name, _)| !name.eq_ignore_ascii_case(missing))
                .cloned()
                .collect();
            let filtered = WebhookPayload::new(headers, b"{}".to_vec());
            let error = extract_headers(&filtered).unwrap_err();
            assert_eq!(error.public_detail(), Some(reason));
            assert_eq!(error.http_status(), 400);

            let blank = webhook_payload(&[("PAYPAL-TRANSMISSION-ID", " ")], b"{}");
            assert!(extract_headers(&blank).is_err());
        }
    }

    #[test]
    fn builds_the_official_verify_request() {
        let payload = signed_payload();
        let headers = extract_headers(&payload).unwrap();
        let event = json!({ "id": "WH-1", "event_type": "PAYMENT.CAPTURE.COMPLETED" });
        let request = build_verify_request("1JE4291016473214C", &headers, &event);
        assert_eq!(request["auth_algo"], "SHA256withRSA");
        assert_eq!(request["cert_url"], "https://api.paypal.com/cert.pem");
        assert_eq!(
            request["transmission_id"],
            "69cd13f0-d67a-11e5-baa3-778b53f4ae55"
        );
        assert_eq!(request["transmission_sig"], "signature-value");
        assert_eq!(request["transmission_time"], "2016-02-18T20:01:35Z");
        assert_eq!(request["webhook_id"], "1JE4291016473214C");
        assert_eq!(request["webhook_event"], event);
    }

    #[test]
    fn normalizes_capture_completed() {
        let value = json!({
            "id": "WH-2WE669878H127391W-3YM43211FL417032U",
            "event_type": "PAYMENT.CAPTURE.COMPLETED",
            "create_time": "2023-06-25T21:41:28Z",
            "resource_type": "capture",
            "resource": {
                "id": "3C679366HH908993F",
                "status": "COMPLETED",
                "custom_id": "ORDER-1",
                "amount": { "currency_code": "USD", "value": "99.00" },
                "create_time": "2023-06-25T21:39:15Z"
            }
        });
        let event = parse_event(&value, AccountId::from("default")).unwrap();
        assert_eq!(event.event_type, PaymentEventType::Succeeded);
        assert_eq!(event.dedupe_key, "WH-2WE669878H127391W-3YM43211FL417032U");
        assert_eq!(event.order_id.as_deref(), Some("ORDER-1"));
        assert_eq!(
            event.provider_payment_id.as_deref(),
            Some("3C679366HH908993F")
        );
        assert_eq!(event.provider_status.as_deref(), Some("COMPLETED"));
        assert_eq!(
            event.amount,
            Some(webx_pay::Money::new(webx_pay::Currency::USD, 9900))
        );
        assert_eq!(event.occurred_at, 1_687_729_288);
        assert_eq!(event.account, Some(AccountId::from("default")));
        assert_eq!(event.raw, value);
    }

    #[test]
    fn normalizes_declined_and_denied_captures_as_failed() {
        for event_type in ["PAYMENT.CAPTURE.DECLINED", "PAYMENT.CAPTURE.DENIED"] {
            let value = json!({
                "id": "WH-1",
                "event_type": event_type,
                "create_time": "2023-06-25T21:41:28Z",
                "resource": {
                    "id": "CAP1",
                    "status": "DECLINED",
                    "custom_id": "ORDER-1",
                    "amount": { "currency_code": "USD", "value": "1.00" }
                }
            });
            let event = parse_event(&value, AccountId::default_account()).unwrap();
            assert_eq!(event.event_type, PaymentEventType::Failed);
            assert_eq!(event.dedupe_key, "WH-1");
        }
    }

    #[test]
    fn normalizes_capture_and_sale_refunds() {
        let capture_refund = json!({
            "id": "WH-3",
            "event_type": "PAYMENT.CAPTURE.REFUNDED",
            "resource": {
                "id": "3C679366HH908993F",
                "status": "REFUNDED",
                "custom_id": "ORDER-1",
                "amount": { "currency_code": "USD", "value": "5.00" }
            }
        });
        let event = parse_event(&capture_refund, AccountId::default_account()).unwrap();
        assert_eq!(event.event_type, PaymentEventType::RefundSucceeded);
        assert_eq!(
            event.amount,
            Some(webx_pay::Money::new(webx_pay::Currency::USD, 500))
        );
        assert_eq!(event.order_id.as_deref(), Some("ORDER-1"));

        // Legacy Payments v1 sale shape: `{total, currency}` + `state`.
        let sale_refund = json!({
            "id": "WH-4",
            "event_type": "PAYMENT.SALE.REFUNDED",
            "resource": {
                "id": "SALE1",
                "state": "refunded",
                "amount": { "total": "7.47", "currency": "USD" }
            }
        });
        let event = parse_event(&sale_refund, AccountId::default_account()).unwrap();
        assert_eq!(event.event_type, PaymentEventType::RefundSucceeded);
        assert_eq!(event.provider_status.as_deref(), Some("refunded"));
        assert_eq!(
            event.amount,
            Some(webx_pay::Money::new(webx_pay::Currency::USD, 747))
        );
    }

    #[test]
    fn normalizes_checkout_order_approved() {
        let value = json!({
            "id": "WH-5",
            "event_type": "CHECKOUT.ORDER.APPROVED",
            "create_time": "2024-05-07T18:46:15Z",
            "resource": {
                "id": "8K832279M55989614",
                "status": "APPROVED",
                "purchase_units": [{
                    "reference_id": "ORDER-2",
                    "custom_id": "ORDER-2",
                    "amount": { "currency_code": "USD", "value": "100.00" }
                }]
            }
        });
        let event = parse_event(&value, AccountId::default_account()).unwrap();
        assert_eq!(event.event_type, PaymentEventType::AuthorizationSucceeded);
        assert_eq!(event.order_id.as_deref(), Some("ORDER-2"));
        assert_eq!(
            event.provider_payment_id.as_deref(),
            Some("8K832279M55989614")
        );
        assert_eq!(
            event.amount,
            Some(webx_pay::Money::new(webx_pay::Currency::USD, 10000))
        );
    }

    #[test]
    fn unknown_event_types_stay_unknown() {
        let value = json!({
            "id": "WH-6",
            "event_type": "CUSTOMER.DISPUTE.CREATED",
            "resource": { "id": "DISPUTE-1" }
        });
        let event = parse_event(&value, AccountId::default_account()).unwrap();
        assert_eq!(
            event.event_type,
            PaymentEventType::Unknown("CUSTOMER.DISPUTE.CREATED".to_string())
        );
        assert_eq!(event.order_id, None);
        assert_eq!(event.amount, None);
        assert!(event.occurred_at > 0);
    }

    #[test]
    fn rejects_events_without_an_id() {
        let error = parse_event(
            &json!({ "event_type": "PAYMENT.CAPTURE.COMPLETED" }),
            AccountId::default_account(),
        )
        .unwrap_err();
        assert!(matches!(error, PaymentError::Serialization(_)));
    }

    #[test]
    fn event_name_helpers_are_stable() {
        assert_eq!(
            map_event_type("PAYMENT.CAPTURE.COMPLETED"),
            PaymentEventType::Succeeded
        );
        assert_eq!(
            map_event_type("CHECKOUT.ORDER.APPROVED"),
            PaymentEventType::AuthorizationSucceeded
        );
        assert_eq!(
            map_event_type("SOMETHING.ELSE"),
            PaymentEventType::Unknown("SOMETHING.ELSE".to_string())
        );
    }
}
