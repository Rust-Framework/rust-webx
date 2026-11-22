//! PayPal Orders v2 / Payments v2 wire models and shared-model conversion.
//!
//! Request bodies are built as JSON values (the wire shape is small and the
//! adapter deliberately keeps provider-specific fields out of `webx_pay`);
//! responses are parsed into typed structs before they reach
//! [`webx_pay::PaymentIntent`] / [`webx_pay::Refund`].

use serde::Deserialize;
use serde_json::{json, Map, Value};
use webx_pay::{
    AccountId, ClientAction, CreatePayment, Money, PaymentError, PaymentFlow, PaymentIntent,
    PaymentStatus, ProviderId, Refund, RefundCommand, RefundStatus,
};

use crate::mapping::{
    map_capture_status, map_order_status, map_refund_status, money_from_object, parse_rfc3339_secs,
};
use crate::PROVIDER;

/// PayPal money object (`{"currency_code": "USD", "value": "99.00"}`).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct AmountJson {
    #[serde(default)]
    pub(crate) currency_code: String,
    #[serde(default)]
    pub(crate) value: String,
}

impl AmountJson {
    pub(crate) fn to_money(&self) -> Option<Money> {
        money_from_object(&json!({
            "currency_code": self.currency_code,
            "value": self.value,
        }))
    }
}

fn amount_json(amount: Money) -> Value {
    json!({
        "currency_code": amount.currency.code(),
        "value": amount.to_major_string(),
    })
}

/// A HATEOAS link (`approve`/`payer-action` are the payer approval links).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct LinkJson {
    #[serde(default)]
    pub(crate) href: String,
    #[serde(default)]
    pub(crate) rel: String,
}

/// PayPal order resource (`POST/GET /v2/checkout/orders/{id}`).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct PaypalOrder {
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) intent: Option<String>,
    #[serde(default)]
    pub(crate) create_time: Option<String>,
    #[serde(default)]
    pub(crate) purchase_units: Vec<PurchaseUnit>,
    #[serde(default)]
    pub(crate) links: Vec<LinkJson>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct PurchaseUnit {
    #[serde(default)]
    pub(crate) reference_id: Option<String>,
    #[serde(default)]
    pub(crate) custom_id: Option<String>,
    #[serde(default)]
    pub(crate) amount: Option<AmountJson>,
    #[serde(default)]
    pub(crate) payments: Option<Payments>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Payments {
    #[serde(default)]
    pub(crate) captures: Vec<CaptureJson>,
    #[serde(default)]
    pub(crate) authorizations: Vec<AuthorizationJson>,
    #[serde(default)]
    pub(crate) refunds: Vec<RefundJson>,
}

/// A captured payment (`PAYMENT.CAPTURE.*` resource / capture endpoints).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CaptureJson {
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) amount: Option<AmountJson>,
}

impl CaptureJson {
    pub(crate) fn parse(value: &Value) -> Result<Self, PaymentError> {
        serde_json::from_value(value.clone()).map_err(|error| {
            PaymentError::Serialization(format!("invalid paypal capture response: {error}"))
        })
    }

    pub(crate) fn money(&self) -> Option<Money> {
        self.amount.as_ref().and_then(AmountJson::to_money)
    }
}

/// An authorized payment (`POST /v2/payments/authorizations/{id}/void`).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct AuthorizationJson {
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) status: String,
}

/// A refund (`POST /v2/payments/captures/{id}/refund`,
/// `GET /v2/payments/refunds/{id}`).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct RefundJson {
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) amount: Option<AmountJson>,
    #[serde(default)]
    pub(crate) custom_id: Option<String>,
    #[serde(default)]
    pub(crate) invoice_id: Option<String>,
    #[serde(default)]
    pub(crate) create_time: Option<String>,
    #[serde(default)]
    pub(crate) update_time: Option<String>,
}

impl RefundJson {
    pub(crate) fn parse(value: &Value) -> Result<Self, PaymentError> {
        serde_json::from_value(value.clone()).map_err(|error| {
            PaymentError::Serialization(format!("invalid paypal refund response: {error}"))
        })
    }

    fn created_at_secs(&self) -> Option<i64> {
        self.create_time.as_deref().and_then(parse_rfc3339_secs)
    }

    fn updated_at_secs(&self) -> Option<i64> {
        self.update_time
            .as_deref()
            .and_then(parse_rfc3339_secs)
            .or_else(|| self.created_at_secs())
    }
}

impl PaypalOrder {
    pub(crate) fn parse(value: &Value) -> Result<Self, PaymentError> {
        serde_json::from_value(value.clone()).map_err(|error| {
            PaymentError::Serialization(format!("invalid paypal order response: {error}"))
        })
    }

    fn first_purchase_unit(&self) -> Option<&PurchaseUnit> {
        self.purchase_units.first()
    }

    /// Merchant order id carried by `purchase_units[].custom_id`/`reference_id`.
    pub(crate) fn merchant_order_id(&self) -> Option<String> {
        let unit = self.first_purchase_unit()?;
        unit.custom_id
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .or(unit.reference_id.as_ref())
            .cloned()
    }

    /// Order amount from the first purchase unit.
    pub(crate) fn money(&self) -> Option<Money> {
        self.first_purchase_unit()?
            .amount
            .as_ref()
            .and_then(AmountJson::to_money)
    }

    /// First capture reported under the first purchase unit.
    pub(crate) fn first_capture(&self) -> Option<&CaptureJson> {
        self.first_purchase_unit()?
            .payments
            .as_ref()?
            .captures
            .first()
    }

    /// First authorization reported under the first purchase unit.
    pub(crate) fn first_authorization(&self) -> Option<&AuthorizationJson> {
        self.first_purchase_unit()?
            .payments
            .as_ref()?
            .authorizations
            .first()
    }

    /// Capture id that can be refunded (a completed or partially refunded
    /// capture).
    pub(crate) fn refundable_capture_id(&self) -> Option<&str> {
        self.first_purchase_unit()?
            .payments
            .as_ref()?
            .captures
            .iter()
            .find(|capture| matches!(capture.status.as_str(), "COMPLETED" | "PARTIALLY_REFUNDED"))
            .map(|capture| capture.id.as_str())
            .filter(|id| !id.is_empty())
    }

    /// Payer approval link (`payer-action` for `payment_source.paypal` orders,
    /// legacy `approve` for bare orders).
    pub(crate) fn payer_action_link(&self) -> Option<&str> {
        self.links
            .iter()
            .find(|link| {
                link.rel.eq_ignore_ascii_case("payer-action")
                    || link.rel.eq_ignore_ascii_case("approve")
            })
            .map(|link| link.href.as_str())
            .filter(|href| !href.is_empty())
    }

    pub(crate) fn created_at_secs(&self) -> Option<i64> {
        self.create_time.as_deref().and_then(parse_rfc3339_secs)
    }

    /// Amount of the first settled capture.
    pub(crate) fn captured_amount(&self) -> Option<Money> {
        let capture = self
            .first_purchase_unit()?
            .payments
            .as_ref()?
            .captures
            .iter()
            .find(|capture| {
                matches!(
                    capture.status.as_str(),
                    "COMPLETED" | "PARTIALLY_REFUNDED" | "REFUNDED"
                )
            })?;
        capture.money()
    }

    /// Sum of completed refunds reported on the order, falling back to a fully
    /// refunded capture amount.
    pub(crate) fn refunded_amount(&self) -> Option<Money> {
        let mut total: Option<Money> = None;
        if let Some(payments) = self
            .first_purchase_unit()
            .and_then(|unit| unit.payments.as_ref())
        {
            for refund in payments
                .refunds
                .iter()
                .filter(|refund| refund.status == "COMPLETED")
            {
                let Some(amount) = refund.amount.as_ref().and_then(AmountJson::to_money) else {
                    continue;
                };
                total = Some(match total {
                    Some(current) => match current.checked_add(amount) {
                        Ok(sum) => sum,
                        Err(_) => current,
                    },
                    None => amount,
                });
            }
        }
        if total.is_some() {
            return total;
        }
        let capture = self
            .first_purchase_unit()?
            .payments
            .as_ref()?
            .captures
            .first()?;
        if capture.status == "REFUNDED" {
            capture.money()
        } else {
            None
        }
    }
}

/// Shared status view for an order plus its first capture.
fn order_status(order: &PaypalOrder) -> PaymentStatus {
    let base = map_order_status(&order.status, order.intent.as_deref());
    let Some(capture) = order.first_capture() else {
        return base;
    };
    let capture_status = map_capture_status(&capture.status);
    match base {
        PaymentStatus::Succeeded if capture_status != PaymentStatus::Unknown => capture_status,
        PaymentStatus::Unknown => capture_status,
        _ => base,
    }
}

/// Convert a PayPal order resource into the shared payment intent.
pub(crate) fn order_to_intent(
    order: &PaypalOrder,
    account: AccountId,
    order_id_hint: Option<&str>,
    amount_hint: Option<Money>,
    request_id: Option<String>,
) -> Result<PaymentIntent, PaymentError> {
    if order.id.trim().is_empty() {
        return Err(PaymentError::Serialization(
            "paypal order response has no id".to_string(),
        ));
    }
    let provider_payment_id = order.id.clone();
    let order_id = order
        .merchant_order_id()
        .or_else(|| order_id_hint.map(str::to_string))
        .unwrap_or_else(|| provider_payment_id.clone());
    let amount = order.money().or(amount_hint).ok_or_else(|| {
        PaymentError::Serialization("paypal order response has no amount".to_string())
    })?;
    let status = order_status(order);
    let mut intent = PaymentIntent::new(
        PROVIDER,
        account,
        order_id,
        amount,
        status,
        order.status.clone(),
    );
    intent.provider_payment_id = Some(provider_payment_id);
    intent.amount_captured = order.captured_amount();
    intent.amount_refunded = order.refunded_amount();
    intent.created_at = order.created_at_secs().unwrap_or(0);
    intent.request_id = request_id;
    Ok(intent)
}

/// Convert a PayPal refund resource into the shared refund.
pub(crate) fn refund_to_refund(
    refund: &RefundJson,
    account: AccountId,
    order_id_hint: Option<&str>,
    refund_id_hint: Option<&str>,
    request_id: Option<String>,
) -> Result<Refund, PaymentError> {
    if refund.id.trim().is_empty() {
        return Err(PaymentError::Serialization(
            "paypal refund response has no id".to_string(),
        ));
    }
    let amount = refund
        .amount
        .as_ref()
        .and_then(AmountJson::to_money)
        .ok_or_else(|| {
            PaymentError::Serialization("paypal refund response has no amount".to_string())
        })?;
    let refund_id = refund_id_hint
        .map(str::to_string)
        .or_else(|| refund.invoice_id.clone())
        .unwrap_or_else(|| refund.id.clone());
    let order_id = order_id_hint
        .map(str::to_string)
        .or_else(|| refund.custom_id.clone())
        .unwrap_or_default();
    let status = map_refund_status(&refund.status);
    let created_at = refund.created_at_secs().unwrap_or(0);
    let succeeded_at = if status == RefundStatus::Succeeded {
        refund.updated_at_secs().filter(|seconds| *seconds > 0)
    } else {
        None
    };
    Ok(Refund {
        provider: PROVIDER,
        account,
        order_id,
        refund_id,
        provider_refund_id: Some(refund.id.clone()),
        status,
        provider_status: refund.status.clone(),
        amount,
        created_at,
        succeeded_at,
        request_id,
    })
}

/// Build the `POST /v2/checkout/orders` request body.
///
/// The payer-approval redirect URLs travel in
/// `payment_source.paypal.experience_context` (the current Orders v2 shape; the
/// deprecated top-level `application_context` is not used).
pub(crate) fn build_create_order_body(request: &CreatePayment, flow: &PaymentFlow) -> Value {
    let intent = if request.capture == webx_pay::CaptureMode::Manual {
        "AUTHORIZE"
    } else {
        "CAPTURE"
    };
    let mut unit = Map::new();
    unit.insert("reference_id".to_string(), json!(request.order_id));
    unit.insert("custom_id".to_string(), json!(request.order_id));
    unit.insert("amount".to_string(), amount_json(request.amount));
    if !request.description.trim().is_empty() {
        unit.insert("description".to_string(), json!(request.description));
    }

    let mut body = Map::new();
    body.insert("intent".to_string(), json!(intent));
    body.insert("purchase_units".to_string(), json!([Value::Object(unit)]));
    if let PaymentFlow::Redirect {
        success_url,
        cancel_url,
    } = flow
    {
        let mut experience = Map::new();
        experience.insert("return_url".to_string(), json!(success_url));
        experience.insert("user_action".to_string(), json!("PAY_NOW"));
        if let Some(cancel_url) = cancel_url {
            experience.insert("cancel_url".to_string(), json!(cancel_url));
        }
        body.insert(
            "payment_source".to_string(),
            json!({ "paypal": { "experience_context": Value::Object(experience) } }),
        );
    }
    Value::Object(body)
}

/// Build the `POST /v2/payments/authorizations/{id}/capture` request body.
///
/// `final_capture` closes the authorization when the full amount is captured.
pub(crate) fn build_authorization_capture_body(
    amount: Option<Money>,
    final_capture: bool,
) -> Value {
    let mut body = Map::new();
    if let Some(amount) = amount {
        body.insert("amount".to_string(), amount_json(amount));
    }
    body.insert("final_capture".to_string(), json!(final_capture));
    Value::Object(body)
}

/// Build the `POST /v2/checkout/orders/{id}/capture` request body.
///
/// The Orders v2 capture endpoint has no amount field; partial capture is only
/// possible through the authorized-payment capture endpoint.
pub(crate) fn build_order_capture_body() -> Value {
    json!({})
}

/// Build the `POST /v2/payments/captures/{id}/refund` request body.
///
/// `custom_id` carries the merchant order id and `invoice_id` the merchant
/// refund id so `GET /v2/payments/refunds/{id}` can recover both. PayPal caps
/// both identifiers at 127 characters; `note_to_payer` at 255.
pub(crate) fn build_refund_body(request: &RefundCommand) -> Value {
    let mut body = Map::new();
    if let Some(amount) = request.amount {
        body.insert("amount".to_string(), amount_json(amount));
    }
    if request.order_id.len() <= 127 {
        body.insert("custom_id".to_string(), json!(request.order_id));
    }
    if request.refund_id.len() <= 127 {
        body.insert("invoice_id".to_string(), json!(request.refund_id));
    }
    let reason = request.reason.trim();
    if !reason.is_empty() {
        let note: String = reason.chars().take(255).collect();
        body.insert("note_to_payer".to_string(), json!(note));
    }
    Value::Object(body)
}

/// Client-SDK action payload for a created PayPal order.
pub(crate) fn client_sdk_action(provider_payment_id: &str) -> ClientAction {
    ClientAction::ClientSdk {
        provider: ProviderId::PayPal,
        payload: json!({ "order_id": provider_payment_id }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webx_pay::{AccountId, CaptureMode, Currency, PaymentMethod};

    fn payment(flow: PaymentFlow) -> CreatePayment {
        CreatePayment::new(
            "ORDER-1",
            Money::new(Currency::USD, 9900),
            PaymentMethod::PayPal,
            flow,
        )
    }

    fn redirect_flow() -> PaymentFlow {
        PaymentFlow::Redirect {
            success_url: "https://example.com/ok".to_string(),
            cancel_url: Some("https://example.com/cancel".to_string()),
        }
    }

    #[test]
    fn builds_capture_order_with_decimal_amount() {
        let request =
            payment(PaymentFlow::ClientSdk { return_url: None }).with_description("One shirt");
        let body = build_create_order_body(&request, &request.flow);
        assert_eq!(body["intent"], "CAPTURE");
        assert!(body.get("payment_source").is_none());
        assert_eq!(body["purchase_units"][0]["reference_id"], "ORDER-1");
        assert_eq!(body["purchase_units"][0]["custom_id"], "ORDER-1");
        assert_eq!(body["purchase_units"][0]["description"], "One shirt");
        assert_eq!(
            body["purchase_units"][0]["amount"],
            serde_json::json!({ "currency_code": "USD", "value": "99.00" })
        );
    }

    #[test]
    fn builds_authorize_order_for_manual_capture_with_redirect_urls() {
        let request = payment(redirect_flow()).with_manual_capture();
        let body = build_create_order_body(&request, &request.flow);
        assert_eq!(body["intent"], "AUTHORIZE");
        assert_eq!(
            body["payment_source"]["paypal"]["experience_context"]["return_url"],
            "https://example.com/ok"
        );
        assert_eq!(
            body["payment_source"]["paypal"]["experience_context"]["cancel_url"],
            "https://example.com/cancel"
        );
        assert_eq!(
            body["payment_source"]["paypal"]["experience_context"]["user_action"],
            "PAY_NOW"
        );
    }

    #[test]
    fn renders_currency_exponents_as_decimal_strings() {
        let mut request = payment(PaymentFlow::QrCode);
        request.amount = Money::new(Currency::JPY, 1_000);
        assert_eq!(
            build_create_order_body(&request, &request.flow)["purchase_units"][0]["amount"]
                ["value"],
            "1000"
        );

        request.amount = Money::new(Currency::KWD, 1234);
        assert_eq!(
            build_create_order_body(&request, &request.flow)["purchase_units"][0]["amount"]
                ["value"],
            "1.234"
        );

        request.amount = Money::new(Currency::USD, 1);
        assert_eq!(
            build_create_order_body(&request, &request.flow)["purchase_units"][0]["amount"]
                ["value"],
            "0.01"
        );
    }

    #[test]
    fn builds_authorization_capture_body() {
        assert_eq!(
            build_authorization_capture_body(Some(Money::new(Currency::USD, 1_000)), false),
            serde_json::json!({
                "amount": { "currency_code": "USD", "value": "10.00" },
                "final_capture": false
            })
        );
        assert_eq!(
            build_authorization_capture_body(None, true),
            serde_json::json!({ "final_capture": true })
        );
        assert_eq!(build_order_capture_body(), serde_json::json!({}));
    }

    #[test]
    fn builds_refund_body_with_traceable_ids() {
        let request = RefundCommand::new("PAYPALORDER1", "REFUND-1")
            .with_amount(Money::new(Currency::USD, 500))
            .with_reason("customer request");
        let body = build_refund_body(&request);
        assert_eq!(
            body["amount"],
            serde_json::json!({ "currency_code": "USD", "value": "5.00" })
        );
        assert_eq!(body["custom_id"], "PAYPALORDER1");
        assert_eq!(body["invoice_id"], "REFUND-1");
        assert_eq!(body["note_to_payer"], "customer request");
    }

    #[test]
    fn refund_body_omits_optional_fields_and_handles_full_refund() {
        let request = RefundCommand::new("PAYPALORDER1", "REFUND-1");
        let body = build_refund_body(&request);
        assert!(body.get("amount").is_none());
        assert!(body.get("note_to_payer").is_none());
        assert_eq!(body["custom_id"], "PAYPALORDER1");
    }

    #[test]
    fn refund_body_truncates_long_reason() {
        let request = RefundCommand::new("PAYPALORDER1", "REFUND-1").with_reason("x".repeat(400));
        let body = build_refund_body(&request);
        assert_eq!(body["note_to_payer"].as_str().unwrap().len(), 255);
    }

    #[test]
    fn maps_a_captured_order_to_an_intent() {
        let value = serde_json::json!({
            "id": "5O190127TN364715T",
            "status": "COMPLETED",
            "intent": "CAPTURE",
            "create_time": "2024-10-14T21:29:26Z",
            "purchase_units": [{
                "reference_id": "ORDER-1",
                "custom_id": "ORDER-1",
                "amount": { "currency_code": "USD", "value": "99.00" },
                "payments": { "captures": [{
                    "id": "3C679366HH908993F",
                    "status": "COMPLETED",
                    "amount": { "currency_code": "USD", "value": "99.00" },
                    "create_time": "2024-10-14T21:30:00Z"
                }]}
            }],
            "links": [{ "href": "https://api.paypal.com/x", "rel": "self" }]
        });
        let order = PaypalOrder::parse(&value).unwrap();
        let intent = order_to_intent(
            &order,
            AccountId::from("default"),
            None,
            None,
            Some("debug-1".to_string()),
        )
        .unwrap();
        assert_eq!(intent.provider, ProviderId::PayPal);
        assert_eq!(intent.order_id, "ORDER-1");
        assert_eq!(
            intent.provider_payment_id.as_deref(),
            Some("5O190127TN364715T")
        );
        assert_eq!(intent.status, PaymentStatus::Succeeded);
        assert_eq!(intent.provider_status, "COMPLETED");
        assert_eq!(intent.amount, Money::new(Currency::USD, 9900));
        assert_eq!(
            intent.amount_captured,
            Some(Money::new(Currency::USD, 9900))
        );
        assert_eq!(intent.created_at, 1_728_941_366);
        assert_eq!(intent.request_id.as_deref(), Some("debug-1"));
        assert!(intent.client_action.is_none());
    }

    #[test]
    fn pending_capture_turns_a_completed_order_into_processing() {
        let value = serde_json::json!({
            "id": "5O190127TN364715T",
            "status": "COMPLETED",
            "purchase_units": [{
                "custom_id": "ORDER-1",
                "amount": { "currency_code": "USD", "value": "10.00" },
                "payments": { "captures": [{ "id": "CAP1", "status": "PENDING" }]}
            }]
        });
        let order = PaypalOrder::parse(&value).unwrap();
        let intent =
            order_to_intent(&order, AccountId::default_account(), None, None, None).unwrap();
        assert_eq!(intent.status, PaymentStatus::Processing);
        assert_eq!(intent.amount_captured, None);
    }

    #[test]
    fn refunded_capture_reports_refund_state_and_amounts() {
        let value = serde_json::json!({
            "id": "5O190127TN364715T",
            "status": "COMPLETED",
            "purchase_units": [{
                "custom_id": "ORDER-1",
                "amount": { "currency_code": "USD", "value": "99.00" },
                "payments": {
                    "captures": [{
                        "id": "CAP1",
                        "status": "REFUNDED",
                        "amount": { "currency_code": "USD", "value": "99.00" }
                    }],
                    "refunds": [{
                        "id": "REF1",
                        "status": "COMPLETED",
                        "amount": { "currency_code": "USD", "value": "99.00" }
                    }]
                }
            }]
        });
        let order = PaypalOrder::parse(&value).unwrap();
        let intent =
            order_to_intent(&order, AccountId::default_account(), None, None, None).unwrap();
        assert_eq!(intent.status, PaymentStatus::Refunded);
        assert_eq!(
            intent.amount_refunded,
            Some(Money::new(Currency::USD, 9900))
        );
    }

    #[test]
    fn exposes_the_payer_action_link_and_capture_ids() {
        let value = serde_json::json!({
            "id": "5O190127TN364715T",
            "status": "APPROVED",
            "intent": "AUTHORIZE",
            "links": [
                { "href": "https://www.paypal.com/checkoutnow?token=5O190127TN364715T", "rel": "payer-action" },
                { "href": "https://api.paypal.com/self", "rel": "self" }
            ],
            "purchase_units": [{
                "amount": { "currency_code": "USD", "value": "10.00" },
                "payments": {
                    "authorizations": [{ "id": "AUTH1", "status": "CREATED" }],
                    "captures": [{ "id": "CAP1", "status": "COMPLETED" }]
                }
            }]
        });
        let order = PaypalOrder::parse(&value).unwrap();
        assert_eq!(
            order.payer_action_link(),
            Some("https://www.paypal.com/checkoutnow?token=5O190127TN364715T")
        );
        assert_eq!(order.first_authorization().unwrap().id, "AUTH1");
        assert_eq!(order.refundable_capture_id(), Some("CAP1"));
        let intent =
            order_to_intent(&order, AccountId::default_account(), None, None, None).unwrap();
        assert_eq!(intent.status, PaymentStatus::Authorized);
    }

    #[test]
    fn parses_and_maps_a_refund_resource() {
        let value = serde_json::json!({
            "id": "0K35355239430361V",
            "status": "COMPLETED",
            "amount": { "currency_code": "USD", "value": "1.00" },
            "custom_id": "ORDER-1",
            "invoice_id": "REFUND-1",
            "create_time": "2024-10-14T14:58:34-07:00",
            "update_time": "2024-10-14T14:58:34-07:00"
        });
        let refund = RefundJson::parse(&value).unwrap();
        let mapped = refund_to_refund(
            &refund,
            AccountId::from("default"),
            None,
            None,
            Some("debug-2".to_string()),
        )
        .unwrap();
        assert_eq!(mapped.order_id, "ORDER-1");
        assert_eq!(mapped.refund_id, "REFUND-1");
        assert_eq!(
            mapped.provider_refund_id.as_deref(),
            Some("0K35355239430361V")
        );
        assert_eq!(mapped.status, RefundStatus::Succeeded);
        assert_eq!(mapped.amount, Money::new(Currency::USD, 100));
        assert_eq!(mapped.created_at, 1_728_943_114);
        assert_eq!(mapped.succeeded_at, Some(1_728_943_114));
        assert_eq!(mapped.request_id.as_deref(), Some("debug-2"));
    }

    #[test]
    fn refund_hints_win_over_echoed_ids() {
        let value = serde_json::json!({
            "id": "REF-PROVIDER-1",
            "status": "PENDING",
            "amount": { "currency_code": "USD", "value": "1.00" },
            "custom_id": "ORDER-FROM-PAYPAL",
            "invoice_id": "INVOICE-FROM-PAYPAL"
        });
        let refund = RefundJson::parse(&value).unwrap();
        let mapped = refund_to_refund(
            &refund,
            AccountId::default_account(),
            Some("ORDER-MERCHANT"),
            Some("REFUND-MERCHANT"),
            None,
        )
        .unwrap();
        assert_eq!(mapped.order_id, "ORDER-MERCHANT");
        assert_eq!(mapped.refund_id, "REFUND-MERCHANT");
        assert_eq!(mapped.status, RefundStatus::Pending);
        assert_eq!(mapped.succeeded_at, None);
    }

    #[test]
    fn client_sdk_action_payload_names_the_order() {
        let action = client_sdk_action("5O190127TN364715T");
        assert_eq!(
            action,
            ClientAction::ClientSdk {
                provider: ProviderId::PayPal,
                payload: serde_json::json!({ "order_id": "5O190127TN364715T" }),
            }
        );
    }

    #[test]
    fn manual_capture_mode_is_reflected_in_the_intent_string() {
        let request = payment(PaymentFlow::QrCode).with_manual_capture();
        assert_eq!(request.capture, CaptureMode::Manual);
        assert_eq!(
            build_create_order_body(&request, &request.flow)["intent"],
            "AUTHORIZE"
        );
    }
}
