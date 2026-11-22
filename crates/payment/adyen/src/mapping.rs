//! Adyen wire <-> shared-model mapping and request-body construction.
//!
//! Everything here is pure: request builders take the shared models and produce
//! the exact JSON Adyen expects (Checkout API v71), and response helpers turn
//! Adyen `resultCode`/`action` values into the provider-neutral model. Keeping
//! this apart from the transport makes the wire format unit-testable offline.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use webx_pay::{
    CaptureMode, ClientAction, Currency, Money, PaymentError, PaymentFlow, PaymentMethod,
    PaymentStatus, ProviderId, RefundStatus,
};

use crate::{METADATA_CHANNEL, METADATA_PAYMENT_METHOD};

/// Provider id owned by this adapter.
pub(crate) const PROVIDER: ProviderId = ProviderId::Adyen;

/// Adyen metadata limits per request (`/payments` reference): 20 pairs, 20
/// characters per key, 80 characters per value.
const MAX_METADATA_ENTRIES: usize = 20;
const MAX_METADATA_KEY_LEN: usize = 20;
const MAX_METADATA_VALUE_LEN: usize = 80;

/// Map an Adyen `resultCode` to the shared payment state.
///
/// `capture` describes how the account captures funds: after an automatic
/// capture an `Authorised` payment is complete (`Succeeded`), while in manual
/// capture mode it is only authorized (`Authorized`). Passing `None` (for
/// example when re-querying through `/payments/details`) keeps `Authorised` as
/// `Authorized`, because this adapter cannot know the account capture mode from
/// the query alone.
///
/// Caveat: the mapping trusts the **requested** capture mode. If the merchant
/// account is configured for manual capture in the Customer Area while the
/// caller asked for an automatic capture (no `additionalData.manualCapture`),
/// Adyen returns `Authorised` for an authorization that is not yet captured;
/// the `AuthorisationSucceeded`/`CAPTURE` webhooks remain authoritative.
pub fn map_result_code(result_code: &str, capture: Option<CaptureMode>) -> PaymentStatus {
    match result_code {
        "Authorised" => match capture {
            Some(CaptureMode::Automatic) => PaymentStatus::Succeeded,
            _ => PaymentStatus::Authorized,
        },
        "PartiallyAuthorised" => PaymentStatus::Authorized,
        "Received" | "Pending" => PaymentStatus::Processing,
        "RedirectShopper" | "ChallengeShopper" | "IdentifyShopper" | "PresentToShopper" => {
            PaymentStatus::RequiresAction
        }
        "Cancelled" => PaymentStatus::Canceled,
        "Refused" | "Error" => PaymentStatus::Failed,
        // AuthenticationFinished / AuthenticationNotRequired belong to the
        // standalone authentication-only flow, which is not modeled here.
        _ => PaymentStatus::Unknown,
    }
}

/// Map the `status` of a capture/cancel modification response (`received`).
pub(crate) fn map_modification_status(status: &str) -> PaymentStatus {
    match status {
        "received" => PaymentStatus::Processing,
        "failed" => PaymentStatus::Failed,
        "cancelled" | "canceled" => PaymentStatus::Canceled,
        _ => PaymentStatus::Unknown,
    }
}

/// Map the `status` of a refund modification response (`received`).
pub(crate) fn map_refund_status(status: &str) -> RefundStatus {
    match status {
        "received" => RefundStatus::Pending,
        "failed" => RefundStatus::Failed,
        "cancelled" | "canceled" => RefundStatus::Canceled,
        _ => RefundStatus::Unknown,
    }
}

/// Build `Money` from Adyen's integer minor-unit `value` and ISO currency.
pub(crate) fn money_from(value: Option<i64>, currency: Option<&str>) -> Option<Money> {
    let value = value?;
    let currency = Currency::new(currency?).ok()?;
    Some(Money::new(currency, value))
}

/// Read `amount` from a Checkout response, falling back to the requested amount.
pub(crate) fn money_from_response(value: &Value, fallback: Option<&Money>) -> Option<Money> {
    let amount = money_from(
        value
            .get("amount")
            .and_then(|amount| amount.get("value"))
            .and_then(Value::as_i64),
        value
            .get("amount")
            .and_then(|amount| amount.get("currency"))
            .and_then(Value::as_str),
    );
    amount.or_else(|| fallback.copied())
}

pub(crate) fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// The Adyen `paymentMethod.type` for a shared method.
///
/// The adapter models only the type plus, when the caller supplies
/// [`METADATA_PAYMENT_METHOD`], the rest of the payment-method object. Card
/// details (encrypted fields/SDK tokens) never reach this server.
pub(crate) fn payment_method_type(method: &PaymentMethod) -> Result<&str, PaymentError> {
    match method {
        PaymentMethod::Card => Ok("scheme"),
        PaymentMethod::ApplePay => Ok("applepay"),
        PaymentMethod::GooglePay => Ok("paywithgoogle"),
        PaymentMethod::PayPal => Ok("paypal"),
        PaymentMethod::AlipayPage
        | PaymentMethod::AlipayWap
        | PaymentMethod::AlipayApp
        | PaymentMethod::AlipayQr => Ok("alipay"),
        PaymentMethod::WechatNative
        | PaymentMethod::WechatJsapi
        | PaymentMethod::WechatApp
        | PaymentMethod::WechatH5 => Err(PaymentError::unsupported(PROVIDER, "wechat_pay")),
        PaymentMethod::Other(name) => {
            let name = name.trim();
            let name = name.strip_prefix("adyen:").unwrap_or(name);
            if name.is_empty() {
                return Err(PaymentError::validation(
                    "adyen payment method type must not be empty",
                ));
            }
            Ok(name)
        }
    }
}

/// Build the `paymentMethod` object: the mapped `type` plus every field of the
/// optional `adyen_payment_method` JSON metadata object (which may add encrypted
/// card fields, SDK tokens or issuer details).
pub(crate) fn payment_method_object(
    method: &PaymentMethod,
    metadata: &BTreeMap<String, String>,
) -> Result<Value, PaymentError> {
    let kind = payment_method_type(method)?;
    let mut object = Map::new();
    object.insert("type".to_string(), Value::String(kind.to_string()));
    if let Some(raw) = metadata.get(METADATA_PAYMENT_METHOD) {
        let parsed: Value = serde_json::from_str(raw).map_err(|error| {
            PaymentError::validation(format!(
                "metadata `{METADATA_PAYMENT_METHOD}` must be a JSON object: {error}"
            ))
        })?;
        let extra = parsed.as_object().ok_or_else(|| {
            PaymentError::validation(format!(
                "metadata `{METADATA_PAYMENT_METHOD}` must be a JSON object"
            ))
        })?;
        for (key, value) in extra {
            object.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(object))
}

/// Adyen `channel` value for the flow.
///
/// Adyen only accepts `Web`, `iOS` and `Android`; the shared [`PaymentFlow`]
/// cannot distinguish iOS from Android, so `App` defaults to `Android` and
/// callers can override it with the `adyen_channel` metadata key.
pub(crate) fn channel_for(
    flow: &PaymentFlow,
    metadata: &BTreeMap<String, String>,
) -> Result<String, PaymentError> {
    if let Some(override_value) = metadata.get(METADATA_CHANNEL) {
        return match override_value.trim().to_ascii_lowercase().as_str() {
            "web" => Ok("Web".to_string()),
            "ios" => Ok("iOS".to_string()),
            "android" => Ok("Android".to_string()),
            other => Err(PaymentError::validation(format!(
                "metadata `{METADATA_CHANNEL}` must be `Web`, `iOS` or `Android` (got `{other}`)"
            ))),
        };
    }
    Ok(match flow {
        PaymentFlow::App { .. } => "Android".to_string(),
        _ => "Web".to_string(),
    })
}

/// The `returnUrl` for the flow, validated when present.
pub(crate) fn return_url_for_flow(flow: &PaymentFlow) -> Result<Option<&str>, PaymentError> {
    let (url, mandatory) = match flow {
        PaymentFlow::ClientSdk { return_url } => (return_url.as_deref(), false),
        PaymentFlow::Redirect { success_url, .. } => (Some(success_url.as_str()), true),
        PaymentFlow::QrCode => (None, false),
        PaymentFlow::App { return_url } => (return_url.as_deref(), false),
    };
    match url {
        Some(url) => {
            validate_return_url(url)?;
            Ok(Some(url))
        }
        None if mandatory => Err(PaymentError::validation(
            "adyen redirect checkout requires a non-empty success_url",
        )),
        None => Ok(None),
    }
}

fn validate_return_url(url: &str) -> Result<(), PaymentError> {
    let url = url.trim();
    if url.is_empty() {
        return Err(PaymentError::validation("returnUrl must not be empty"));
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(PaymentError::validation(
            "returnUrl must be an absolute http(s) URL",
        ));
    }
    if url.chars().any(char::is_control) {
        return Err(PaymentError::validation(
            "returnUrl must not contain control characters",
        ));
    }
    Ok(())
}

/// Build the `/payments` request body.
pub(crate) fn build_create_body(
    request: &webx_pay::CreatePayment,
    merchant_account: &str,
) -> Result<Value, PaymentError> {
    let mut body = Map::new();
    body.insert(
        "amount".to_string(),
        json!({
            "value": request.amount.minor,
            "currency": request.amount.currency.code(),
        }),
    );
    body.insert("reference".to_string(), json!(request.order_id));
    body.insert("merchantAccount".to_string(), json!(merchant_account));
    body.insert(
        "paymentMethod".to_string(),
        payment_method_object(&request.method, &request.metadata)?,
    );
    body.insert(
        "channel".to_string(),
        json!(channel_for(&request.flow, &request.metadata)?),
    );
    if let Some(return_url) = return_url_for_flow(&request.flow)? {
        body.insert("returnUrl".to_string(), json!(return_url));
        // Adyen defaults to Ecommerce; set it explicitly for redirect-style
        // flows so the request is self-describing.
        body.insert("shopperInteraction".to_string(), json!("Ecommerce"));
    }
    if let Some(client_ip) = request.client_ip {
        body.insert("shopperIP".to_string(), json!(client_ip.to_string()));
    }
    if let Some(payer_ref) = &request.payer_ref {
        body.insert("shopperReference".to_string(), json!(payer_ref));
    }
    let metadata = outgoing_metadata(&request.metadata)?;
    if !metadata.is_empty() {
        body.insert("metadata".to_string(), Value::Object(metadata));
    }
    if request.capture == CaptureMode::Manual {
        body.insert(
            "additionalData".to_string(),
            json!({ "manualCapture": "true" }),
        );
    }
    Ok(Value::Object(body))
}

/// Build the `/payments/details` request body.
pub(crate) fn build_details_body(
    details: &Value,
    payment_data: Option<&str>,
) -> Result<Value, PaymentError> {
    let details = details.as_object().ok_or_else(|| {
        PaymentError::validation("adyen `details` must be a JSON object of payment details")
    })?;
    let mut body = Map::new();
    body.insert("details".to_string(), Value::Object(details.clone()));
    if let Some(payment_data) = payment_data {
        body.insert("paymentData".to_string(), json!(payment_data));
    }
    Ok(Value::Object(body))
}

/// Build a `/payments/{pspReference}/captures` request body.
pub(crate) fn build_capture_body(merchant_account: &str, amount: &Money, reference: &str) -> Value {
    json!({
        "merchantAccount": merchant_account,
        "amount": {
            "value": amount.minor,
            "currency": amount.currency.code(),
        },
        "reference": reference,
    })
}

/// Build a `/payments/{pspReference}/cancels` request body.
pub(crate) fn build_cancel_body(merchant_account: &str, reference: &str) -> Value {
    json!({
        "merchantAccount": merchant_account,
        "reference": reference,
    })
}

/// Build a `/payments/{pspReference}/refunds` request body.
///
/// Adyen's `amount` is required: the refund API refunds a specified amount and
/// this adapter has no way to look up the captured amount.
pub(crate) fn build_refund_body(
    merchant_account: &str,
    amount: &Money,
    reference: &str,
) -> Result<Value, PaymentError> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Err(PaymentError::validation(
            "adyen refund reference (refund_id) must not be empty",
        ));
    }
    if reference.chars().any(char::is_control) || reference.len() > 80 {
        return Err(PaymentError::validation(
            "adyen refund reference must be at most 80 characters without control characters",
        ));
    }
    if !amount.is_positive() {
        return Err(PaymentError::validation("refund amount must be positive"));
    }
    Ok(json!({
        "merchantAccount": merchant_account,
        "reference": reference,
        "amount": {
            "value": amount.minor,
            "currency": amount.currency.code(),
        },
    }))
}

/// Placeholder amount for responses that do not echo an amount.
///
/// Adyen's `/cancels` response only returns references and `status: received`;
/// the shared [`webx_pay::PaymentIntent`] requires an amount, so cancel
/// acknowledgements use ISO 4217 `XXX` (no currency) with zero minor units. The
/// authoritative amount stays with the stored payment and the `CANCELLATION`
/// webhook.
pub(crate) fn unknown_amount() -> Money {
    Money::new(
        Currency::new("XXX").expect("XXX is a valid ISO 4217 currency code"),
        0,
    )
}

/// Metadata that may be sent to Adyen.
///
/// Adapter instruction keys (`adyen_*`) are consumed locally and never forwarded;
/// Adyen's documented metadata limits are enforced here (20 entries, 20-char
/// keys, 80-char values) instead of being discovered through a 422.
fn outgoing_metadata(
    metadata: &BTreeMap<String, String>,
) -> Result<Map<String, Value>, PaymentError> {
    let mut outgoing = Map::new();
    for (key, value) in metadata {
        if is_reserved_metadata_key(key) {
            continue;
        }
        if key.len() > MAX_METADATA_KEY_LEN {
            return Err(PaymentError::validation(format!(
                "adyen metadata keys must be at most {MAX_METADATA_KEY_LEN} characters (got `{key}`)"
            )));
        }
        if value.len() > MAX_METADATA_VALUE_LEN {
            return Err(PaymentError::validation(format!(
                "adyen metadata values must be at most {MAX_METADATA_VALUE_LEN} characters (key `{key}`)"
            )));
        }
        outgoing.insert(key.clone(), Value::String(value.clone()));
    }
    if outgoing.len() > MAX_METADATA_ENTRIES {
        return Err(PaymentError::validation(format!(
            "adyen metadata supports at most {MAX_METADATA_ENTRIES} entries"
        )));
    }
    Ok(outgoing)
}

/// Whether a metadata key is an adapter instruction instead of Adyen metadata.
pub(crate) fn is_reserved_metadata_key(key: &str) -> bool {
    matches!(
        key,
        crate::METADATA_PAYMENT_METHOD
            | crate::METADATA_DETAILS
            | crate::METADATA_PAYMENT_DATA
            | crate::METADATA_CHANNEL
    )
}

/// Map an Adyen `action` object to the next client action for `flow`.
///
/// * redirect-type actions become [`ClientAction::Redirect`] for every browser
///   flow;
/// * `qrCode` actions become [`ClientAction::QrCode`] for [`PaymentFlow::QrCode`];
/// * everything else (3-D Secure 2 fingerprint/challenge, vouchers, native SDK
///   actions) keeps the raw action JSON so the provider's Web/App SDK can handle
///   it, wrapped with the `pspReference` that later calls need.
pub(crate) fn client_action(flow: &PaymentFlow, response: &Value) -> Option<ClientAction> {
    let action = response.get("action")?;
    if action.is_null() {
        return None;
    }
    let action_type = string_field(action, "type").unwrap_or_default();
    let psp_reference = string_field(response, "pspReference");
    match flow {
        PaymentFlow::Redirect { .. } | PaymentFlow::ClientSdk { .. } => {
            if action_type == "redirect" {
                return string_field(action, "url").map(|url| ClientAction::Redirect {
                    url: url.to_string(),
                });
            }
            Some(ClientAction::ClientSdk {
                provider: PROVIDER,
                payload: action_payload(action, psp_reference),
            })
        }
        PaymentFlow::QrCode => {
            if action_type == "qrCode" {
                return string_field(action, "qrCodeData").map(|content| ClientAction::QrCode {
                    content: content.to_string(),
                });
            }
            Some(ClientAction::ClientSdk {
                provider: PROVIDER,
                payload: action_payload(action, psp_reference),
            })
        }
        PaymentFlow::App { .. } => Some(ClientAction::AppOrder {
            provider: PROVIDER,
            payload: action_payload(action, psp_reference),
        }),
    }
}

fn action_payload(action: &Value, psp_reference: Option<&str>) -> Value {
    let mut payload = Map::new();
    payload.insert("action".to_string(), action.clone());
    if let Some(psp_reference) = psp_reference {
        payload.insert("pspReference".to_string(), json!(psp_reference));
    }
    Value::Object(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{METADATA_CHANNEL, METADATA_PAYMENT_METHOD};

    fn create_request(flow: PaymentFlow) -> webx_pay::CreatePayment {
        webx_pay::CreatePayment::new(
            "ORDER-1",
            Money::new(Currency::EUR, 9900),
            PaymentMethod::Other("ideal".into()),
            flow,
        )
    }

    #[test]
    fn maps_every_documented_result_code() {
        let automatic = Some(CaptureMode::Automatic);
        let manual = Some(CaptureMode::Manual);
        assert_eq!(
            map_result_code("Authorised", automatic),
            PaymentStatus::Succeeded
        );
        assert_eq!(
            map_result_code("Authorised", manual),
            PaymentStatus::Authorized
        );
        assert_eq!(
            map_result_code("Authorised", None),
            PaymentStatus::Authorized
        );
        assert_eq!(
            map_result_code("PartiallyAuthorised", automatic),
            PaymentStatus::Authorized
        );
        assert_eq!(
            map_result_code("Received", automatic),
            PaymentStatus::Processing
        );
        assert_eq!(
            map_result_code("Pending", automatic),
            PaymentStatus::Processing
        );
        for code in ["RedirectShopper", "ChallengeShopper", "IdentifyShopper"] {
            assert_eq!(
                map_result_code(code, automatic),
                PaymentStatus::RequiresAction,
                "{code}"
            );
        }
        assert_eq!(
            map_result_code("PresentToShopper", automatic),
            PaymentStatus::RequiresAction
        );
        assert_eq!(
            map_result_code("Cancelled", automatic),
            PaymentStatus::Canceled
        );
        assert_eq!(map_result_code("Refused", automatic), PaymentStatus::Failed);
        assert_eq!(map_result_code("Error", automatic), PaymentStatus::Failed);
        assert_eq!(
            map_result_code("AuthenticationFinished", automatic),
            PaymentStatus::Unknown
        );
        assert_eq!(
            map_result_code("SomeFutureCode", automatic),
            PaymentStatus::Unknown
        );
        assert_eq!(
            map_modification_status("received"),
            PaymentStatus::Processing
        );
        assert_eq!(map_modification_status("failed"), PaymentStatus::Failed);
        assert_eq!(map_refund_status("received"), RefundStatus::Pending);
        assert_eq!(map_refund_status("failed"), RefundStatus::Failed);
    }

    #[test]
    fn builds_the_redirect_payment_body() {
        let mut request = create_request(PaymentFlow::Redirect {
            success_url: "https://merchant.example.com/return?order=ORDER-1".into(),
            cancel_url: Some("https://merchant.example.com/cancel".into()),
        });
        request
            .metadata
            .insert("order_id".to_string(), "ORDER-1".to_string());
        request.metadata.insert(
            METADATA_PAYMENT_METHOD.to_string(),
            r#"{"type":"ideal","issuer":"fixtureIssuer"}"#.to_string(),
        );
        request
            .metadata
            .insert(METADATA_CHANNEL.to_string(), "web".to_string());
        let body = build_create_body(&request, "FixtureMerchant").unwrap();
        assert_eq!(
            body,
            json!({
                "amount": { "value": 9900, "currency": "EUR" },
                "reference": "ORDER-1",
                "merchantAccount": "FixtureMerchant",
                "paymentMethod": { "type": "ideal", "issuer": "fixtureIssuer" },
                "channel": "Web",
                "returnUrl": "https://merchant.example.com/return?order=ORDER-1",
                "shopperInteraction": "Ecommerce",
                "metadata": { "order_id": "ORDER-1" }
            })
        );
    }

    #[test]
    fn builds_the_client_sdk_manual_capture_body() {
        let mut request = create_request(PaymentFlow::ClientSdk {
            return_url: Some("https://merchant.example.com/return".into()),
        });
        request.method = PaymentMethod::Card;
        request.payer_ref = Some("shopper-fixture-1".into());
        request = request.with_manual_capture();
        let body = build_create_body(&request, "FixtureMerchant").unwrap();
        assert_eq!(
            body,
            json!({
                "amount": { "value": 9900, "currency": "EUR" },
                "reference": "ORDER-1",
                "merchantAccount": "FixtureMerchant",
                "paymentMethod": { "type": "scheme" },
                "channel": "Web",
                "returnUrl": "https://merchant.example.com/return",
                "shopperInteraction": "Ecommerce",
                "shopperReference": "shopper-fixture-1",
                "additionalData": { "manualCapture": "true" }
            })
        );
    }

    #[test]
    fn app_flow_defaults_to_android_and_can_be_overridden() {
        let mut request = create_request(PaymentFlow::App {
            return_url: Some("fixture-app://return".into()),
        });
        let error = build_create_body(&request, "FixtureMerchant").unwrap_err();
        // App return URLs are custom schemes, not http(s); the shared model
        // documents them as "where to return", so only http(s) is accepted.
        assert_eq!(error.http_status(), 400);

        request.flow = PaymentFlow::App { return_url: None };
        let body = build_create_body(&request, "FixtureMerchant").unwrap();
        assert_eq!(body["channel"], json!("Android"));
        request
            .metadata
            .insert(METADATA_CHANNEL.to_string(), "iOS".to_string());
        let body = build_create_body(&request, "FixtureMerchant").unwrap();
        assert_eq!(body["channel"], json!("iOS"));

        request
            .metadata
            .insert(METADATA_CHANNEL.to_string(), "Desktop".to_string());
        assert!(build_create_body(&request, "FixtureMerchant").is_err());
    }

    #[test]
    fn payment_methods_map_to_adyen_types() {
        assert_eq!(payment_method_type(&PaymentMethod::Card).unwrap(), "scheme");
        assert_eq!(
            payment_method_type(&PaymentMethod::GooglePay).unwrap(),
            "paywithgoogle"
        );
        assert_eq!(
            payment_method_type(&PaymentMethod::Other("adyen:ideal".into())).unwrap(),
            "ideal"
        );
        assert!(matches!(
            payment_method_type(&PaymentMethod::WechatNative),
            Err(PaymentError::Unsupported { .. })
        ));
        assert!(payment_method_type(&PaymentMethod::Other(" ".into())).is_err());
    }

    #[test]
    fn adyen_metadata_limits_are_enforced() {
        let mut request = create_request(PaymentFlow::QrCode);
        for index in 0..MAX_METADATA_ENTRIES {
            request
                .metadata
                .insert(format!("k{index}"), "v".to_string());
        }
        assert!(build_create_body(&request, "merchant").is_ok());
        request
            .metadata
            .insert(format!("k{MAX_METADATA_ENTRIES}"), "v".to_string());
        assert!(build_create_body(&request, "merchant").is_err());

        let mut request = create_request(PaymentFlow::QrCode);
        request.metadata.insert("k".repeat(21), "v".to_string());
        assert!(build_create_body(&request, "merchant").is_err());

        let mut request = create_request(PaymentFlow::QrCode);
        request.metadata.insert("key".to_string(), "v".repeat(81));
        assert!(build_create_body(&request, "merchant").is_err());
    }

    #[test]
    fn builds_modification_bodies() {
        let money = Money::new(Currency::EUR, 2500);
        assert_eq!(
            build_capture_body("FixtureMerchant", &money, "ORDER-1"),
            json!({
                "merchantAccount": "FixtureMerchant",
                "amount": { "value": 2500, "currency": "EUR" },
                "reference": "ORDER-1"
            })
        );
        assert_eq!(
            build_cancel_body("FixtureMerchant", "ORDER-1"),
            json!({ "merchantAccount": "FixtureMerchant", "reference": "ORDER-1" })
        );
        assert_eq!(
            build_refund_body("FixtureMerchant", &money, "REFUND-1").unwrap(),
            json!({
                "merchantAccount": "FixtureMerchant",
                "reference": "REFUND-1",
                "amount": { "value": 2500, "currency": "EUR" }
            })
        );
        assert!(build_refund_body("FixtureMerchant", &money, " ").is_err());
        assert!(build_refund_body("FixtureMerchant", &Money::new(Currency::EUR, 0), "R").is_err());
        assert_eq!(
            unknown_amount(),
            Money::new(Currency::new("XXX").unwrap(), 0)
        );
    }

    #[test]
    fn builds_details_bodies() {
        assert_eq!(
            build_details_body(
                &json!({ "redirectResult": "fixture-redirect-result" }),
                Some("fixture-payment-data")
            )
            .unwrap(),
            json!({
                "details": { "redirectResult": "fixture-redirect-result" },
                "paymentData": "fixture-payment-data"
            })
        );
        assert!(build_details_body(&json!("not an object"), None).is_err());
    }

    #[test]
    fn maps_redirect_actions_to_redirect_and_3ds_actions_to_sdk_payloads() {
        let sdk_flow = PaymentFlow::ClientSdk { return_url: None };
        let redirect = json!({
            "pspReference": "7914073381342284",
            "resultCode": "RedirectShopper",
            "action": {
                "type": "redirect",
                "url": "https://issuer.example.com/redirect",
                "method": "GET"
            }
        });
        assert_eq!(
            client_action(&sdk_flow, &redirect),
            Some(ClientAction::Redirect {
                url: "https://issuer.example.com/redirect".into()
            })
        );

        let three_ds = json!({
            "pspReference": "7914073381342284",
            "resultCode": "IdentifyShopper",
            "action": {
                "type": "threeDS2",
                "subtype": "fingerprint",
                "token": "fixture-token",
                "paymentData": "fixture-payment-data"
            }
        });
        let action = client_action(&sdk_flow, &three_ds).unwrap();
        assert_eq!(
            action,
            ClientAction::ClientSdk {
                provider: PROVIDER,
                payload: json!({
                    "action": three_ds["action"].clone(),
                    "pspReference": "7914073381342284"
                })
            }
        );

        let qr_flow = PaymentFlow::QrCode;
        let qr = json!({
            "action": { "type": "qrCode", "qrCodeData": "fixture-qr-payload" }
        });
        assert_eq!(
            client_action(&qr_flow, &qr),
            Some(ClientAction::QrCode {
                content: "fixture-qr-payload".into()
            })
        );
        assert_eq!(client_action(&qr_flow, &json!({})), None);
    }

    #[test]
    fn reads_amount_with_fallback() {
        let fallback = Money::new(Currency::USD, 100);
        assert_eq!(
            money_from_response(
                &json!({ "amount": { "value": 9900, "currency": "EUR" } }),
                Some(&fallback)
            ),
            Some(Money::new(Currency::EUR, 9900))
        );
        assert_eq!(
            money_from_response(&json!({}), Some(&fallback)),
            Some(fallback)
        );
        assert_eq!(money_from_response(&json!({}), None), None);
        assert_eq!(money_from(Some(1), Some("us")), None);
    }
}
