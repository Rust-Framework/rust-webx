//! Alipay asynchronous notification (异步通知) verification and normalization.
//!
//! Notifications are `application/x-www-form-urlencoded` POSTs. The RSA2
//! signature covers every parameter except `sign` and `sign_type`, sorted by
//! key (the official SDK's "V1" rule). Alipay expects the literal body
//! `success` as the acknowledgement.
//!
//! De-duplication contract: Alipay has no event id, so the stable key is
//! `out_trade_no:trade_status:trade_no` (no timestamp component, stable across
//! provider retries).

use async_trait::async_trait;
use rsa::PublicKey;
use serde_json::{Map, Value};
use webx_core::action::Response;
use webx_core::security::constant_time_eq;
use webx_pay::{
    text_success_ack, AccountSet, IPaymentWebhookVerifier, PaymentError, PaymentEvent,
    PaymentScope, ProviderId, WebhookPayload,
};

use crate::mapping::{event_type_for_trade_status, money_cny};
use crate::provider::AlipayAccount;
use crate::sign;
use crate::time;
use crate::PROVIDER;

/// Verify the RSA2 signature of an Alipay asynchronous notification.
///
/// `params` are the decoded form fields. `alipay_public_key_pem` accepts
/// Alipay's public key (public-key mode) **or** the content of its public-key
/// certificate (certificate mode), because both hold the same RSA public key.
pub fn verify_notification_signature(
    params: &[(String, String)],
    alipay_public_key_pem: &str,
) -> Result<(), PaymentError> {
    let public_key = sign::parse_public_key(alipay_public_key_pem)
        .or_else(|_| sign::certificate_public_key(alipay_public_key_pem))
        .map_err(|detail| PaymentError::config(PROVIDER, format!("alipay public key: {detail}")))?;
    verify_notification_with_key(params, &public_key)
}

/// Verify the RSA2 signature with an already parsed public key.
pub(crate) fn verify_notification_with_key(
    params: &[(String, String)],
    public_key: &PublicKey,
) -> Result<(), PaymentError> {
    if let Some(sign_type) = field(params, "sign_type") {
        if !sign::sign_type_is_rsa2(sign_type) {
            return Err(PaymentError::signature(PROVIDER, "unsupported sign_type"));
        }
    }
    let signature = field(params, "sign")
        .ok_or_else(|| PaymentError::signature(PROVIDER, "missing signature"))?;
    let content = sign::notify_sign_content(params);
    sign::rsa2_verify(&content, signature, public_key)
        .map_err(|reason| PaymentError::signature(PROVIDER, reason))
}

/// Verify the notification signature plus its application binding and
/// normalize the payload into a [`PaymentEvent`].
pub(crate) fn normalize_notification(
    params: &[(String, String)],
    account: &AlipayAccount,
) -> Result<PaymentEvent, PaymentError> {
    // The RSA signature proves Alipay sent the fields, not which application
    // they belong to: several applications share Alipay's signing keys, so the
    // notified `app_id` must match this account's.
    let app_id = field(params, "app_id")
        .or_else(|| field(params, "auth_app_id"))
        .ok_or_else(|| PaymentError::signature(PROVIDER, "missing app_id"))?;
    if !constant_time_eq(app_id.as_bytes(), account.app_id.as_bytes()) {
        return Err(PaymentError::signature(PROVIDER, "app_id mismatch"));
    }

    let required = |name: &'static str| {
        field(params, name).ok_or_else(|| {
            PaymentError::Serialization(format!("alipay notification has no {name}"))
        })
    };
    let trade_status = required("trade_status")?;
    let out_trade_no = required("out_trade_no")?;
    let trade_no = field(params, "trade_no").unwrap_or_default();
    let amount = money_cny(required("total_amount")?).ok_or_else(|| {
        PaymentError::Serialization("alipay notification has an invalid total_amount".to_string())
    })?;

    let dedupe_key = format!("{out_trade_no}:{trade_status}:{trade_no}");
    let mut event = PaymentEvent::new(
        PROVIDER,
        event_type_for_trade_status(trade_status),
        dedupe_key,
    );
    event.account = Some(account.id.clone());
    event.provider_event_id = None;
    event.provider_status = Some(trade_status.to_string());
    event.order_id = Some(out_trade_no.to_string());
    event.provider_payment_id = (!trade_no.is_empty()).then(|| trade_no.to_string());
    event.amount = Some(amount);
    event.occurred_at = field(params, "notify_time")
        .and_then(time::parse_datetime)
        .unwrap_or(0);
    // Alipay does not tell the receiver whether the notification came from the
    // live or the sandbox environment; the adapter's account classification is
    // the only source for that.
    event.livemode = None;
    event.raw = params_to_json(params);
    Ok(event)
}

fn field<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
}

fn params_to_json(params: &[(String, String)]) -> Value {
    let mut object = Map::new();
    for (key, value) in params {
        object.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(object)
}

/// Verify Alipay callbacks for every configured account.
#[derive(Clone)]
pub struct AlipayWebhookVerifier {
    accounts: Result<AccountSet<AlipayAccount>, String>,
}

impl AlipayWebhookVerifier {
    pub(crate) fn new(accounts: Result<AccountSet<AlipayAccount>, String>) -> Self {
        Self { accounts }
    }
}

#[async_trait]
impl IPaymentWebhookVerifier for AlipayWebhookVerifier {
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
                "alipay webhook requires the account path segment when multiple accounts are configured",
            ));
        }
        let account = accounts.resolve(account)?;
        let params = payload.form()?;
        verify_notification_with_key(&params, &account.public_key)?;
        normalize_notification(&params, account)
    }

    fn ack(&self) -> Response {
        text_success_ack()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;
    use crate::test_support::scope;
    use crate::transport;

    fn payload(params: &[(String, String)]) -> WebhookPayload {
        WebhookPayload::new(
            Vec::<(String, String)>::new(),
            transport::encode_form(params).into_bytes(),
        )
    }

    fn verifier() -> AlipayWebhookVerifier {
        AlipayWebhookVerifier::new(test_support::options().build_accounts())
    }

    fn signed(params: Vec<(String, String)>) -> Vec<(String, String)> {
        test_support::signed_notification(params)
    }

    #[tokio::test]
    async fn verifies_and_normalizes_a_success_notification() {
        let params = signed(test_support::successful_notification());
        let event = verifier()
            .verify(&scope(), &payload(&params))
            .await
            .expect("notification verifies");
        assert_eq!(event.provider, PROVIDER);
        assert_eq!(event.event_type, webx_pay::PaymentEventType::Succeeded);
        assert_eq!(event.provider_status.as_deref(), Some("TRADE_SUCCESS"));
        assert_eq!(event.order_id.as_deref(), Some("ORDER-1"));
        assert_eq!(
            event.provider_payment_id.as_deref(),
            Some("2023111522001440000000000001")
        );
        assert_eq!(event.provider_event_id, None);
        assert_eq!(event.amount, Some(webx_pay::Money::cny(9900)));
        assert_eq!(
            event.dedupe_key,
            "ORDER-1:TRADE_SUCCESS:2023111522001440000000000001"
        );
        assert_eq!(event.occurred_at, 1_700_000_000);
        assert_eq!(event.livemode, None);
        assert_eq!(
            event.account.as_ref().map(|account| account.as_str()),
            Some("default")
        );
        assert_eq!(event.raw["out_trade_no"], "ORDER-1");
    }

    #[tokio::test]
    async fn dedupe_key_is_stable_across_retries() {
        let params = signed(test_support::successful_notification());
        let first = verifier()
            .verify(&scope(), &payload(&params))
            .await
            .unwrap();
        // A retried notification differs only in notify fields, not trade data.
        let mut retried = test_support::successful_notification();
        for (key, value) in retried.iter_mut() {
            if key == "notify_time" {
                *value = "2023-11-15 06:20:00".to_string();
            }
        }
        let second = verifier()
            .verify(&scope(), &payload(&signed(retried)))
            .await
            .unwrap();
        assert_eq!(first.dedupe_key, second.dedupe_key);
    }

    #[tokio::test]
    async fn rejects_tampered_and_unsigned_notifications() {
        let params = signed(test_support::successful_notification());
        let tampered: Vec<(String, String)> = params
            .iter()
            .map(|(key, value)| {
                if key == "total_amount" {
                    (key.clone(), "0.01".to_string())
                } else {
                    (key.clone(), value.clone())
                }
            })
            .collect();
        let error = verifier()
            .verify(&scope(), &payload(&tampered))
            .await
            .unwrap_err();
        assert_eq!(error.public_detail(), Some("signature mismatch"));

        let unsigned = test_support::successful_notification();
        let error = verifier()
            .verify(&scope(), &payload(&unsigned))
            .await
            .unwrap_err();
        assert_eq!(error.public_detail(), Some("missing signature"));
    }

    #[tokio::test]
    async fn rejects_app_id_mismatch_after_a_valid_signature() {
        let mut params = test_support::successful_notification();
        for (key, value) in params.iter_mut() {
            if key == "app_id" {
                *value = "2021000000009999".to_string();
            }
        }
        let error = verifier()
            .verify(&scope(), &payload(&signed(params)))
            .await
            .unwrap_err();
        assert_eq!(error.public_detail(), Some("app_id mismatch"));
    }

    #[tokio::test]
    async fn rejects_unsupported_sign_types() {
        let mut params = test_support::successful_notification();
        for (key, value) in params.iter_mut() {
            if key == "sign_type" {
                *value = "RSA".to_string();
            }
        }
        let error = verifier()
            .verify(&scope(), &payload(&signed(params)))
            .await
            .unwrap_err();
        assert_eq!(error.public_detail(), Some("unsupported sign_type"));
    }

    #[tokio::test]
    async fn closed_trade_maps_to_canceled_event() {
        let mut params = test_support::successful_notification();
        for (key, value) in params.iter_mut() {
            if key == "trade_status" {
                *value = "TRADE_CLOSED".to_string();
            }
        }
        let event = verifier()
            .verify(&scope(), &payload(&signed(params)))
            .await
            .unwrap();
        assert_eq!(event.event_type, webx_pay::PaymentEventType::Canceled);
        assert_eq!(event.provider_status.as_deref(), Some("TRADE_CLOSED"));
    }

    #[tokio::test]
    async fn multiple_accounts_require_the_account_segment() {
        let options = test_support::options().with_account(
            "second",
            "2021000000000002",
            test_support::merchant_private_key_pem(),
            crate::options::AlipayKeyMode::public_key(test_support::alipay_public_key_pem()),
        );
        let verifier = AlipayWebhookVerifier::new(options.build_accounts());
        let params = signed(test_support::successful_notification());
        let error = verifier
            .verify(&scope(), &payload(&params))
            .await
            .unwrap_err();
        assert_eq!(error.http_status(), 400);
        assert!(error.to_string().contains("account path segment"));

        let event = verifier
            .verify(&scope().account("default"), &payload(&params))
            .await
            .unwrap();
        assert_eq!(
            event.account.as_ref().map(|id| id.as_str()),
            Some("default")
        );
    }

    #[test]
    fn public_helper_verifies_with_a_pem_key_or_certificate() {
        let params = signed(test_support::successful_notification());
        assert!(
            verify_notification_signature(&params, test_support::alipay_public_key_pem()).is_ok()
        );
        assert!(verify_notification_signature(
            &params,
            &test_support::certificates().alipay_public_cert_pem
        )
        .is_ok());
        assert!(verify_notification_signature(&params, "not a key").is_err());
    }
}
