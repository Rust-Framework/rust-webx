//! Callback notification verification, resource decryption and normalization.
//!
//! WeChat Pay callbacks are a JSON envelope
//! (`id`, `create_time`, `event_type`, `resource_type`, `resource`) whose
//! `resource.ciphertext` is `AEAD_AES_256_GCM` encrypted with the APIv3 key.
//! The signature over `TIMESTAMP\nNONCE\nBODY\n` is carried in the
//! `Wechatpay-Signature` / `Wechatpay-Timestamp` / `Wechatpay-Nonce` /
//! `Wechatpay-Serial` headers, exactly like API responses.
//!
//! The normalized event keeps the provider's `event_type`, `trade_state` /
//! `refund_status` (as `provider_status`), the merchant order id, the WeChat
//! transaction id, the amount in fen and the notification id as `dedupe_key`.
//! `raw` is the outer notification envelope: the encrypted resource is not
//! persisted there and the decrypted business payload is represented by the
//! normalized fields instead, so payer identifiers (`openid`) do not leak into
//! event-bus consumers through `raw`.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use webx_core::action::Response;
use webx_pay::{
    json_success_ack, AccountSet, IPaymentWebhookVerifier, PaymentError, PaymentEvent,
    PaymentEventType, PaymentScope, ProviderId, WebhookPayload,
};

use crate::crypto::{decrypt_aead_aes_256_gcm, AEAD_ALGORITHM};
use crate::mapping::{amount_currency, amount_field, money_from_fen, string_field};
use crate::provider::WechatPayAccount;
use crate::sign::{response_signature_message, validate_timestamp, verify_message_base64};
use crate::time::{parse_rfc3339, unix_now};
use crate::transport::WechatPayTransport;
use crate::PROVIDER;

/// Maximum accepted clock skew for callback timestamps (WeChat's documented
/// recommendation is five minutes).
pub const SIGNATURE_TOLERANCE_SECONDS: i64 = 300;

/// Payment succeeded notification type.
const TRANSACTION_SUCCESS: &str = "TRANSACTION.SUCCESS";
/// Refund succeeded notification type.
const REFUND_SUCCESS: &str = "REFUND.SUCCESS";
/// Refund abnormal (bank failure) notification type.
const REFUND_ABNORMAL: &str = "REFUND.ABNORMAL";
/// Refund closed notification type.
const REFUND_CLOSED: &str = "REFUND.CLOSED";

/// Verifies WeChat Pay callbacks for every configured merchant account.
///
/// One verifier holds all accounts; callbacks carry the account in the route
/// (`/webhooks/payments/wechatpay/{account}`) because every account has its own
/// APIv3 key and platform keys. With a single account the account parameter may
/// be omitted.
#[derive(Clone)]
pub struct WechatPayWebhookVerifier {
    accounts: Result<AccountSet<WechatPayAccount>, String>,
    transport: Arc<WechatPayTransport>,
}

impl WechatPayWebhookVerifier {
    pub(crate) fn new(
        accounts: Result<AccountSet<WechatPayAccount>, String>,
        transport: Arc<WechatPayTransport>,
    ) -> Self {
        Self {
            accounts,
            transport,
        }
    }
}

#[async_trait]
impl IPaymentWebhookVerifier for WechatPayWebhookVerifier {
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
                "wechatpay webhook requires the account path segment when multiple accounts are configured",
            ));
        }
        let account = accounts.resolve(account)?;
        let timestamp = payload.header("wechatpay-timestamp").ok_or_else(|| {
            PaymentError::signature(PROVIDER, "missing Wechatpay-Timestamp header")
        })?;
        let nonce = payload
            .header("wechatpay-nonce")
            .ok_or_else(|| PaymentError::signature(PROVIDER, "missing Wechatpay-Nonce header"))?;
        let signature = payload.header("wechatpay-signature").ok_or_else(|| {
            PaymentError::signature(PROVIDER, "missing Wechatpay-Signature header")
        })?;
        let serial = payload
            .header("wechatpay-serial")
            .ok_or_else(|| PaymentError::signature(PROVIDER, "missing Wechatpay-Serial header"))?;
        validate_timestamp(timestamp, unix_now(), SIGNATURE_TOLERANCE_SECONDS)
            .map_err(|reason| PaymentError::signature(PROVIDER, reason))?;
        let key = self.transport.verification_key(account, serial).await?;
        let message = response_signature_message(timestamp, nonce, payload.body());
        verify_message_base64(&key, &message, signature)
            .map_err(|_| PaymentError::signature(PROVIDER, "signature mismatch"))?;
        parse_notification(account, payload.body())
    }

    fn ack(&self) -> Response {
        json_success_ack()
    }
}

/// Decrypt and normalize a verified callback body.
fn parse_notification(
    account: &WechatPayAccount,
    body: &[u8],
) -> Result<PaymentEvent, PaymentError> {
    let envelope: Value = serde_json::from_slice(body).map_err(|error| {
        PaymentError::Serialization(format!("invalid WeChat Pay notification JSON: {error}"))
    })?;
    let id = string_field(&envelope, "id")
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            PaymentError::Serialization("WeChat Pay notification has no id".to_string())
        })?
        .to_string();
    let event_type = string_field(&envelope, "event_type")
        .unwrap_or_default()
        .to_string();
    let resource_type = string_field(&envelope, "resource_type").unwrap_or("encrypt-resource");
    if resource_type != "encrypt-resource" {
        return Err(PaymentError::Serialization(format!(
            "unsupported WeChat Pay notification resource_type `{resource_type}`"
        )));
    }
    let resource = envelope.get("resource").ok_or_else(|| {
        PaymentError::Serialization("WeChat Pay notification has no resource".to_string())
    })?;
    let algorithm = string_field(resource, "algorithm").unwrap_or_default();
    if algorithm != AEAD_ALGORITHM {
        return Err(PaymentError::Serialization(format!(
            "unsupported WeChat Pay notification algorithm `{algorithm}`"
        )));
    }
    let nonce = string_field(resource, "nonce").ok_or_else(|| {
        PaymentError::Serialization("WeChat Pay notification resource has no nonce".to_string())
    })?;
    let associated_data = string_field(resource, "associated_data").unwrap_or_default();
    let ciphertext = string_field(resource, "ciphertext").ok_or_else(|| {
        PaymentError::Serialization(
            "WeChat Pay notification resource has no ciphertext".to_string(),
        )
    })?;
    let plaintext =
        decrypt_aead_aes_256_gcm(&account.api_v3_key, nonce, associated_data, ciphertext).map_err(
            |error| {
                PaymentError::Serialization(format!(
                    "WeChat Pay notification resource decryption failed: {error}"
                ))
            },
        )?;
    let payload: Value = serde_json::from_slice(&plaintext).map_err(|error| {
        PaymentError::Serialization(format!(
            "decrypted WeChat Pay notification is not JSON: {error}"
        ))
    })?;
    normalize_event(&envelope, &payload, &id, &event_type, account)
}

fn normalize_event(
    envelope: &Value,
    payload: &Value,
    notification_id: &str,
    event_type: &str,
    account: &WechatPayAccount,
) -> Result<PaymentEvent, PaymentError> {
    let kind = match event_type {
        TRANSACTION_SUCCESS => PaymentEventType::Succeeded,
        REFUND_SUCCESS => PaymentEventType::RefundSucceeded,
        REFUND_ABNORMAL | REFUND_CLOSED => PaymentEventType::RefundFailed,
        other => PaymentEventType::Unknown(other.to_string()),
    };
    let mut event = PaymentEvent::new(PROVIDER, kind.clone(), notification_id);
    event.account = Some(account.id.clone());
    event.provider_event_id = Some(notification_id.to_string());
    event.order_id = string_field(payload, "out_trade_no").map(str::to_string);
    event.provider_payment_id = string_field(payload, "transaction_id").map(str::to_string);
    event.raw = envelope.clone();
    event.occurred_at = string_field(payload, "success_time")
        .and_then(parse_rfc3339)
        .or_else(|| string_field(envelope, "create_time").and_then(parse_rfc3339))
        .unwrap_or_else(unix_now);

    match &kind {
        PaymentEventType::Succeeded => {
            event.provider_status = string_field(payload, "trade_state").map(str::to_string);
            let total = amount_field(payload, "total").ok_or_else(|| {
                PaymentError::Serialization(
                    "WeChat Pay transaction notification has no amount.total".to_string(),
                )
            })?;
            event.amount = Some(money_from_fen(total));
        }
        PaymentEventType::RefundSucceeded | PaymentEventType::RefundFailed => {
            event.provider_status = string_field(payload, "refund_status").map(str::to_string);
            if let Some(refunded) = amount_field(payload, "refund") {
                event.amount = Some(money_from_fen(refunded));
            } else if let Some(total) = amount_field(payload, "total") {
                event.amount = Some(money_from_fen(total));
            }
            if let Some(currency) = amount_currency(payload) {
                if !currency.eq_ignore_ascii_case("CNY") {
                    return Err(PaymentError::Serialization(format!(
                        "unsupported WeChat Pay refund currency `{currency}`"
                    )));
                }
            }
        }
        _ => {
            event.provider_status = string_field(payload, "trade_state")
                .or_else(|| string_field(payload, "refund_status"))
                .map(str::to_string);
            if let Some(total) = amount_field(payload, "total") {
                event.amount = Some(money_from_fen(total));
            }
        }
    }
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use webx_pay::AccountId;

    use crate::test_support;
    use crate::transport::WechatPayTransport;
    use crate::WechatPayOptions;

    const PUBLIC_KEY_ID: &str = "PUB_KEY_ID_TEST";

    fn accounts(options: &WechatPayOptions) -> AccountSet<WechatPayAccount> {
        options.build_accounts().unwrap()
    }

    fn public_key_verifier() -> (WechatPayWebhookVerifier, WechatPayOptions) {
        let options = WechatPayOptions::new(
            test_support::account_options()
                .with_public_key(PUBLIC_KEY_ID, test_support::key_pair().public_pem.clone()),
        );
        let transport = Arc::new(WechatPayTransport::new(options.http_options()));
        (
            WechatPayWebhookVerifier::new(Ok(accounts(&options)), transport),
            options,
        )
    }

    fn platform_verifier(serial: &str) -> (WechatPayWebhookVerifier, WechatPayOptions) {
        let options = WechatPayOptions::new(test_support::account_options());
        let accounts = accounts(&options);
        let account = accounts.resolve(None).unwrap();
        match &account.key_mode {
            crate::provider::AccountKeyMode::PlatformCertificates(cache) => {
                cache.insert(serial, test_support::key_pair().public.as_ref().clone())
            }
            crate::provider::AccountKeyMode::PublicKey { .. } => unreachable!(),
        }
        let transport = Arc::new(WechatPayTransport::new(options.http_options()));
        (
            WechatPayWebhookVerifier::new(Ok(accounts), transport),
            options,
        )
    }

    fn encrypted_resource(payload: &Value) -> Value {
        let api_v3_key = test_support::api_v3_key();
        let plaintext = serde_json::to_vec(payload).unwrap();
        let ciphertext = crate::crypto::encrypt_aead_aes_256_gcm(
            &api_v3_key,
            "61f9c719728a",
            "transaction",
            &plaintext,
        );
        serde_json::json!({
            "algorithm": "AEAD_AES_256_GCM",
            "original_type": "transaction",
            "nonce": "61f9c719728a",
            "associated_data": "transaction",
            "ciphertext": ciphertext,
        })
    }

    fn notification(event_type: &str, payload: &Value) -> Value {
        serde_json::json!({
            "id": "EV-2018022511223320873",
            "create_time": "2018-06-08T10:34:56+08:00",
            "resource_type": "encrypt-resource",
            "event_type": event_type,
            "summary": "支付成功",
            "resource": encrypted_resource(payload),
        })
    }

    fn signed_payload(body: &[u8], serial: &str, timestamp: i64) -> WebhookPayload {
        let nonce = "593BEC0C930BF1AFEB40B4A08C8FB242";
        let message = response_signature_message(&timestamp.to_string(), nonce, body);
        let signature =
            crate::sign::sign_message_base64(&test_support::key_pair().private, &message).unwrap();
        WebhookPayload::new(
            [
                ("Wechatpay-Timestamp".to_string(), timestamp.to_string()),
                ("Wechatpay-Nonce".to_string(), nonce.to_string()),
                ("Wechatpay-Signature".to_string(), signature),
                ("Wechatpay-Serial".to_string(), serial.to_string()),
            ],
            body.to_vec(),
        )
    }

    async fn verify(
        verifier: &WechatPayWebhookVerifier,
        payload: &WebhookPayload,
    ) -> Result<PaymentEvent, PaymentError> {
        verifier.verify(&PaymentScope::new(PROVIDER), payload).await
    }

    #[tokio::test]
    async fn verifies_and_normalizes_a_transaction_success_notification() {
        let (verifier, _options) = public_key_verifier();
        let value = notification(
            "TRANSACTION.SUCCESS",
            &serde_json::json!({
                "appid": "wxd678efh567hg6787",
                "mchid": "1900000001",
                "out_trade_no": "ORDER-1",
                "transaction_id": "4200001213202301010000000000",
                "trade_type": "NATIVE",
                "trade_state": "SUCCESS",
                "trade_state_desc": "支付成功",
                "success_time": "2018-06-08T10:34:56+08:00",
                "payer": { "openid": "oUpF8uMuAJO_M2pxb1Q9zNjWeS6o" },
                "amount": { "total": 9900, "payer_total": 9900, "currency": "CNY" },
            }),
        );
        let body = serde_json::to_vec(&value).unwrap();
        let event = verify(&verifier, &signed_payload(&body, PUBLIC_KEY_ID, unix_now()))
            .await
            .unwrap();
        assert_eq!(event.provider, PROVIDER);
        assert_eq!(event.event_type, PaymentEventType::Succeeded);
        assert_eq!(event.dedupe_key, "EV-2018022511223320873");
        assert_eq!(
            event.provider_event_id.as_deref(),
            Some("EV-2018022511223320873")
        );
        assert_eq!(event.order_id.as_deref(), Some("ORDER-1"));
        assert_eq!(
            event.provider_payment_id.as_deref(),
            Some("4200001213202301010000000000")
        );
        assert_eq!(event.provider_status.as_deref(), Some("SUCCESS"));
        assert_eq!(event.amount.unwrap().minor, 9900);
        assert_eq!(event.amount.unwrap().currency, webx_pay::Currency::CNY);
        assert_eq!(event.occurred_at, 1_528_425_296);
        assert_eq!(event.account.unwrap().as_str(), AccountId::DEFAULT);
        // `raw` is the outer envelope; the payer openid is not persisted there.
        assert_eq!(event.raw["id"], "EV-2018022511223320873");
        assert!(!event
            .raw
            .to_string()
            .contains("oUpF8uMuAJO_M2pxb1Q9zNjWeS6o"));
    }

    #[tokio::test]
    async fn verifies_and_normalizes_a_refund_success_notification() {
        let (verifier, _options) = public_key_verifier();
        let value = notification(
            "REFUND.SUCCESS",
            &serde_json::json!({
                "mchid": "1900000001",
                "out_trade_no": "ORDER-1",
                "transaction_id": "4200001213202301010000000000",
                "out_refund_no": "REFUND-1",
                "refund_id": "50000000382019052709732678859",
                "refund_status": "SUCCESS",
                "success_time": "2018-06-08T10:34:56+08:00",
                "amount": { "total": 9900, "refund": 1900, "payer_total": 9900, "payer_refund": 1900 },
            }),
        );
        let body = serde_json::to_vec(&value).unwrap();
        let event = verify(&verifier, &signed_payload(&body, PUBLIC_KEY_ID, unix_now()))
            .await
            .unwrap();
        assert_eq!(event.event_type, PaymentEventType::RefundSucceeded);
        assert_eq!(event.order_id.as_deref(), Some("ORDER-1"));
        assert_eq!(event.amount.unwrap().minor, 1900);
        assert_eq!(event.dedupe_key, "EV-2018022511223320873");
    }

    #[tokio::test]
    async fn refund_failure_variants_map_to_refund_failed() {
        let (verifier, _options) = public_key_verifier();
        for event_type in ["REFUND.ABNORMAL", "REFUND.CLOSED"] {
            let value = notification(
                event_type,
                &serde_json::json!({
                    "out_trade_no": "ORDER-1",
                    "refund_status": "ABNORMAL",
                    "amount": { "total": 9900, "refund": 1900 },
                }),
            );
            let body = serde_json::to_vec(&value).unwrap();
            let event = verify(&verifier, &signed_payload(&body, PUBLIC_KEY_ID, unix_now()))
                .await
                .unwrap();
            assert_eq!(event.event_type, PaymentEventType::RefundFailed);
        }
    }

    #[tokio::test]
    async fn unknown_event_types_are_preserved() {
        let (verifier, _options) = public_key_verifier();
        let value = notification(
            "TRANSACTION.SOMETHING_NEW",
            &serde_json::json!({ "out_trade_no": "ORDER-1", "amount": { "total": 1 } }),
        );
        let body = serde_json::to_vec(&value).unwrap();
        let event = verify(&verifier, &signed_payload(&body, PUBLIC_KEY_ID, unix_now()))
            .await
            .unwrap();
        assert_eq!(
            event.event_type,
            PaymentEventType::Unknown("TRANSACTION.SOMETHING_NEW".to_string())
        );
    }

    #[tokio::test]
    async fn tampered_body_signature_and_timestamp_are_rejected() {
        let (verifier, _options) = public_key_verifier();
        let value = notification(
            "TRANSACTION.SUCCESS",
            &serde_json::json!({
                "out_trade_no": "ORDER-1",
                "trade_state": "SUCCESS",
                "amount": { "total": 1 },
            }),
        );
        let body = serde_json::to_vec(&value).unwrap();
        let payload = signed_payload(&body, PUBLIC_KEY_ID, unix_now());
        let mut tampered = body.clone();
        tampered.push(b' ');
        let bad = WebhookPayload::new(payload.headers().to_vec(), tampered);
        let error = verify(&verifier, &bad).await.unwrap_err();
        assert!(matches!(error, PaymentError::SignatureInvalid { .. }));

        let stale = signed_payload(&body, PUBLIC_KEY_ID, unix_now() - 3_600);
        let error = verify(&verifier, &stale).await.unwrap_err();
        assert!(matches!(error, PaymentError::SignatureInvalid { .. }));
        assert_eq!(error.public_detail(), Some("timestamp outside tolerance"));
    }

    #[tokio::test]
    async fn missing_signature_headers_are_rejected() {
        let (verifier, _options) = public_key_verifier();
        let payload = WebhookPayload::new(Vec::<(String, String)>::new(), b"{}".to_vec());
        let error = verify(&verifier, &payload).await.unwrap_err();
        assert!(matches!(error, PaymentError::SignatureInvalid { .. }));
    }

    #[tokio::test]
    async fn tampered_ciphertext_is_rejected_after_signature_verification() {
        let (verifier, _options) = public_key_verifier();
        let mut value = notification(
            "TRANSACTION.SUCCESS",
            &serde_json::json!({ "out_trade_no": "ORDER-1", "amount": { "total": 1 } }),
        );
        // Flip a character in the ciphertext while keeping the envelope JSON
        // valid, then sign the tampered body: the AEAD tag must reject it.
        let ciphertext = value["resource"]["ciphertext"]
            .as_str()
            .unwrap()
            .to_string();
        let mut bytes =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &ciphertext)
                .unwrap();
        bytes[0] ^= 0xFF;
        value["resource"]["ciphertext"] = serde_json::json!(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &bytes
        ));
        let body = serde_json::to_vec(&value).unwrap();
        let error = verify(&verifier, &signed_payload(&body, PUBLIC_KEY_ID, unix_now()))
            .await
            .unwrap_err();
        assert!(matches!(error, PaymentError::Serialization(_)));
    }

    #[tokio::test]
    async fn platform_certificate_mode_fails_closed_for_an_unknown_serial_without_network() {
        let (verifier, _options) = platform_verifier("5157F09EFDC096DE15EBE81A47057A7232F1B8E1");
        let accounts = verifier.accounts.as_ref().unwrap();
        let account = accounts.resolve(None).unwrap();
        // Open the rate-limit window so the fallback refresh is skipped (no
        // network is performed by this test).
        if let crate::provider::AccountKeyMode::PlatformCertificates(cache) = &account.key_mode {
            assert!(cache.try_begin_refresh());
        }
        let body = serde_json::to_vec(&notification(
            "TRANSACTION.SUCCESS",
            &serde_json::json!({
                "out_trade_no": "ORDER-1",
                "trade_state": "SUCCESS",
                "amount": { "total": 1 },
            }),
        ))
        .unwrap();
        let error = verify(
            &verifier,
            &signed_payload(&body, "UNKNOWN-SERIAL", unix_now()),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, PaymentError::Internal(_)));
        assert!(error.to_string().contains("unknown"));
    }

    #[tokio::test]
    async fn platform_certificate_mode_verifies_a_known_serial() {
        let serial = "5157F09EFDC096DE15EBE81A47057A7232F1B8E1";
        let (verifier, _options) = platform_verifier(serial);
        let body = serde_json::to_vec(&notification(
            "TRANSACTION.SUCCESS",
            &serde_json::json!({
                "out_trade_no": "ORDER-1",
                "trade_state": "SUCCESS",
                "amount": { "total": 1 },
            }),
        ))
        .unwrap();
        let event = verify(&verifier, &signed_payload(&body, serial, unix_now()))
            .await
            .unwrap();
        assert_eq!(event.event_type, PaymentEventType::Succeeded);
    }

    #[tokio::test]
    async fn multiple_accounts_require_the_account_path_segment() {
        let options = WechatPayOptions::new(
            test_support::account_options()
                .with_public_key(PUBLIC_KEY_ID, test_support::key_pair().public_pem.clone()),
        )
        .with_account(
            test_support::account_options()
                .with_id("second")
                .with_public_key(PUBLIC_KEY_ID, test_support::key_pair().public_pem.clone()),
        );
        let transport = Arc::new(WechatPayTransport::new(options.http_options()));
        let verifier = WechatPayWebhookVerifier::new(Ok(accounts(&options)), transport);
        let payload = WebhookPayload::new(Vec::<(String, String)>::new(), b"{}".to_vec());
        let error = verifier
            .verify(&PaymentScope::new(PROVIDER), &payload)
            .await
            .unwrap_err();
        assert!(matches!(error, PaymentError::Validation { .. }));
    }

    #[test]
    fn ack_is_the_wechatpay_success_envelope() {
        let (verifier, _options) = public_key_verifier();
        let _: Response = verifier.ack();
    }
}
