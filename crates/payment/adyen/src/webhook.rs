//! Adyen standard-webhook parsing, HMAC verification and event normalization.
//!
//! One POST can carry several notifications
//! (`notificationItems[].NotificationRequestItem`). Every item that carries an
//! `additionalData.hmacSignature` is verified (and a missing signature fails the
//! whole delivery closed); then each item is normalized into a
//! [`PaymentEvent`]. The expected acknowledgement is HTTP 200 with the literal
//! body `[accepted]`.
//!
//! Adyen deduplication contract: `pspReference:eventCode:success`.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use webx_core::action::{ContentResult, Response};
use webx_pay::{
    AccountId, AccountSet, Currency, IPaymentWebhookVerifier, Money, PaymentError, PaymentEvent,
    PaymentEventType, PaymentScope, ProviderId, WebhookPayload,
};

use crate::hmac::{self, NotificationHmacFields};
use crate::provider::AdyenAccount;

const PROVIDER: ProviderId = ProviderId::Adyen;

/// The success response Adyen expects for a delivered standard webhook.
pub const ACK_BODY: &str = "[accepted]";

/// `amount` object of a notification item.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NotificationAmount {
    /// Minor-unit value; Adyen sends it as a number (numeric strings tolerated).
    #[serde(default, deserialize_with = "de_option_i64")]
    pub value: Option<i64>,
    /// ISO 4217 currency code.
    #[serde(default)]
    pub currency: Option<String>,
}

/// The `additionalData` subset the adapter needs.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NotificationAdditionalData {
    /// HMAC-SHA256 signature of the notification item (Base64).
    #[serde(rename = "hmacSignature", default)]
    pub hmac_signature: Option<String>,
}

/// One `NotificationRequestItem` of a standard webhook.
#[derive(Debug, Clone, Deserialize)]
pub struct NotificationItem {
    /// Adyen's 16-character transaction reference. For modifications this is the
    /// modification's own reference.
    #[serde(rename = "pspReference", default)]
    pub psp_reference: String,
    /// The original payment's pspReference for modifications (empty for an
    /// AUTHORISATION).
    #[serde(rename = "originalReference", default)]
    pub original_reference: Option<String>,
    /// Adyen merchant account code; used to route multi-account deliveries.
    #[serde(rename = "merchantAccountCode", default)]
    pub merchant_account_code: Option<String>,
    /// Merchant reference, i.e. the order id sent to `/payments`.
    #[serde(rename = "merchantReference", default)]
    pub merchant_reference: Option<String>,
    /// Amount of the event.
    #[serde(default)]
    pub amount: Option<NotificationAmount>,
    /// Adyen event code (`AUTHORISATION`, `CAPTURE`, `REFUND`, ...).
    #[serde(rename = "eventCode", default)]
    pub event_code: String,
    /// `"true"` / `"false"` (boolean values are normalized to strings).
    #[serde(default, deserialize_with = "de_option_string_boolish")]
    pub success: Option<String>,
    /// Adyen's refusal/capture failure reason, when present.
    #[serde(default)]
    pub reason: Option<String>,
    /// Event timestamp in ISO 8601/RFC 3339, for example
    /// `2019-05-06T17:15:34.121+02:00`.
    #[serde(rename = "eventDate", default)]
    pub event_date: Option<String>,
    /// Additional data (contains the HMAC signature).
    #[serde(rename = "additionalData", default)]
    pub additional_data: Option<NotificationAdditionalData>,
    /// Payment method description/object as sent by Adyen.
    #[serde(rename = "paymentMethod", default)]
    pub payment_method: Option<Value>,
}

impl NotificationItem {
    /// The Base64 signature carried in `additionalData.hmacSignature`.
    pub fn hmac_signature(&self) -> Option<&str> {
        self.additional_data
            .as_ref()
            .and_then(|data| data.hmac_signature.as_deref())
    }

    /// The order id (merchant reference) carried by the event, when present.
    pub fn order_id(&self) -> Option<&str> {
        self.merchant_reference.as_deref()
    }

    /// The payment's pspReference: `originalReference` for modifications, the
    /// item's own `pspReference` for an AUTHORISATION.
    pub fn payment_reference(&self) -> Option<&str> {
        self.original_reference
            .as_deref()
            .filter(|value| !value.is_empty())
            .or(Some(self.psp_reference.as_str()))
            .filter(|value| !value.is_empty())
    }

    /// `success` as a boolean (anything but the literal `true` is failure).
    pub fn success_bool(&self) -> bool {
        self.success
            .as_deref()
            .map(|value| value.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }

    /// `amount.value` rendered the way Adyen signs it (empty when absent).
    fn amount_value_text(&self) -> String {
        self.amount
            .as_ref()
            .and_then(|amount| amount.value)
            .map(|value| value.to_string())
            .unwrap_or_default()
    }

    fn hmac_fields<'a>(&'a self, amount_value: &'a str) -> NotificationHmacFields<'a> {
        NotificationHmacFields {
            psp_reference: self.psp_reference.as_str(),
            original_reference: self.original_reference.as_deref().unwrap_or(""),
            merchant_account_code: self.merchant_account_code.as_deref().unwrap_or(""),
            merchant_reference: self.merchant_reference.as_deref().unwrap_or(""),
            amount_value,
            amount_currency: self
                .amount
                .as_ref()
                .and_then(|amount| amount.currency.as_deref())
                .unwrap_or(""),
            event_code: self.event_code.as_str(),
            success: self.success.as_deref().unwrap_or(""),
        }
    }

    /// The colon-joined HMAC signing string for this item.
    pub fn signing_string(&self) -> String {
        let amount_value = self.amount_value_text();
        hmac::signing_string(&self.hmac_fields(&amount_value))
    }

    /// Calculate the Base64 HMAC-SHA256 signature for this item.
    pub fn signature(&self, hmac_key: &[u8]) -> String {
        let amount_value = self.amount_value_text();
        hmac::sign_notification(&self.hmac_fields(&amount_value), hmac_key)
    }

    /// Verify `signature` for this item in constant time.
    pub fn verify_signature(&self, hmac_key: &[u8], signature: &str) -> Result<(), PaymentError> {
        let amount_value = self.amount_value_text();
        hmac::verify_notification(&self.hmac_fields(&amount_value), hmac_key, signature)
    }

    /// Stable de-duplication key: `pspReference:eventCode:success`.
    pub fn dedupe_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.psp_reference,
            self.event_code,
            self.success.as_deref().unwrap_or("")
        )
    }
}

/// One parsed notification item together with its raw JSON (for audit).
#[derive(Debug, Clone)]
pub struct ParsedNotificationItem {
    /// Typed view of `NotificationRequestItem`.
    pub item: NotificationItem,
    /// The raw `NotificationRequestItem` object.
    pub raw: Value,
}

/// A parsed standard-webhook delivery.
#[derive(Debug, Clone)]
pub struct ParsedNotification {
    /// Adyen's `live` flag, when present.
    pub live: Option<bool>,
    /// `notificationItems`, in wire order; never empty.
    pub items: Vec<ParsedNotificationItem>,
}

/// Parse a standard-webhook body without trusting it.
///
/// The body must be a JSON object with a non-empty `notificationItems` array of
/// `{ "NotificationRequestItem": { ... } }` entries.
pub fn parse_notification(body: &[u8]) -> Result<ParsedNotification, PaymentError> {
    let value: Value = serde_json::from_slice(body).map_err(|error| {
        PaymentError::Serialization(format!("invalid adyen notification JSON: {error}"))
    })?;
    let live = live_flag(value.get("live"));
    let items = value
        .get("notificationItems")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            PaymentError::Serialization(
                "adyen notification is missing `notificationItems`".to_string(),
            )
        })?;
    if items.is_empty() {
        return Err(PaymentError::Serialization(
            "adyen notification contains no notificationItems".to_string(),
        ));
    }
    let mut parsed = Vec::with_capacity(items.len());
    for entry in items {
        let raw = entry
            .get("NotificationRequestItem")
            .ok_or_else(|| {
                PaymentError::Serialization(
                    "adyen notification item is missing `NotificationRequestItem`".to_string(),
                )
            })?
            .clone();
        let item: NotificationItem = serde_json::from_value(raw.clone()).map_err(|error| {
            PaymentError::Serialization(format!("invalid adyen notification item: {error}"))
        })?;
        parsed.push(ParsedNotificationItem { item, raw });
    }
    Ok(ParsedNotification {
        live,
        items: parsed,
    })
}

/// Normalize one verified notification item into a [`PaymentEvent`].
///
/// `raw` is the item's `NotificationRequestItem` JSON; `account` is the resolved
/// merchant account and `live` the delivery's environment flag.
pub fn normalize_event(
    item: &NotificationItem,
    account: AccountId,
    live: Option<bool>,
    raw: Value,
) -> PaymentEvent {
    let event_type = event_type_for(item);
    let mut event = PaymentEvent::new(PROVIDER, event_type, item.dedupe_key());
    event.account = Some(account);
    event.provider_status = Some(item.event_code.clone());
    event.order_id = item.order_id().map(str::to_string);
    event.provider_payment_id = item.payment_reference().map(str::to_string);
    event.amount = item.amount.as_ref().and_then(|amount| {
        let currency = Currency::new(amount.currency.as_deref()?).ok()?;
        Some(Money::new(currency, amount.value?))
    });
    event.occurred_at = item
        .event_date
        .as_deref()
        .and_then(parse_rfc3339_utc_seconds)
        .unwrap_or_else(crate::unix_now);
    event.livemode = live;
    event.raw = raw;
    event
}

fn event_type_for(item: &NotificationItem) -> PaymentEventType {
    let success = item.success_bool();
    match item.event_code.as_str() {
        "AUTHORISATION" => {
            if success {
                PaymentEventType::Succeeded
            } else {
                PaymentEventType::Failed
            }
        }
        "CAPTURE" => {
            if success {
                PaymentEventType::Succeeded
            } else {
                PaymentEventType::Failed
            }
        }
        "REFUND" => {
            if success {
                PaymentEventType::RefundSucceeded
            } else {
                PaymentEventType::RefundFailed
            }
        }
        "REFUND_FAILED" => PaymentEventType::RefundFailed,
        "CANCELLATION" => {
            if success {
                PaymentEventType::Canceled
            } else {
                // A failed cancellation means the authorization still stands; it
                // is not a failed payment.
                PaymentEventType::Unknown("CANCELLATION_FAILED".to_string())
            }
        }
        "CHARGEBACK" | "DISPUTE" | "NOTIFICATION_OF_CHARGEBACK" => PaymentEventType::DisputeOpened,
        other => PaymentEventType::Unknown(other.to_string()),
    }
}

/// Verify every notification item and normalize it into an event.
///
/// The verifier is multi-account aware: when the route does not select an account
/// and several are configured, each item is matched by its
/// `merchantAccountCode`. Any invalid or missing signature rejects the whole
/// delivery.
#[derive(Clone)]
pub struct AdyenWebhookVerifier {
    accounts: Result<AccountSet<AdyenAccount>, String>,
}

impl AdyenWebhookVerifier {
    pub(crate) fn new(accounts: Result<AccountSet<AdyenAccount>, String>) -> Self {
        Self { accounts }
    }

    /// Verify and normalize the complete delivery (all `notificationItems`).
    pub async fn verify_all(
        &self,
        account: Option<&AccountId>,
        payload: &WebhookPayload,
    ) -> Result<Vec<PaymentEvent>, PaymentError> {
        let accounts = self
            .accounts
            .as_ref()
            .map_err(|error| PaymentError::config(PROVIDER, error.clone()))?;
        let parsed = parse_notification(payload.body())?;
        let mut events = Vec::with_capacity(parsed.items.len());
        for entry in parsed.items {
            let account = resolve_webhook_account(accounts, account, &entry.item)?;
            let signature = entry
                .item
                .hmac_signature()
                .ok_or_else(|| PaymentError::signature(PROVIDER, "missing HMAC signature"))?;
            entry.item.verify_signature(&account.hmac_key, signature)?;
            events.push(normalize_event(
                &entry.item,
                account.id.clone(),
                parsed.live,
                entry.raw,
            ));
        }
        Ok(events)
    }
}

#[async_trait]
impl IPaymentWebhookVerifier for AdyenWebhookVerifier {
    fn provider(&self) -> ProviderId {
        PROVIDER
    }

    async fn verify(
        &self,
        scope: &PaymentScope,
        payload: &WebhookPayload,
    ) -> Result<PaymentEvent, PaymentError> {
        let mut events = self.verify_all(scope.account_ref(), payload).await?;
        if events.len() == 1 {
            return Ok(events.remove(0));
        }
        // The shared port can only carry one event. The first event is returned
        // so a single-event delivery behaves exactly; for batches applications
        // must call `verify_all` to process every item.
        events.into_iter().next().ok_or_else(|| {
            PaymentError::Serialization("adyen notification contained no events".to_string())
        })
    }

    fn ack(&self) -> Response {
        Box::new(ContentResult::text(ACK_BODY))
    }
}

fn resolve_webhook_account<'a>(
    accounts: &'a AccountSet<AdyenAccount>,
    requested: Option<&AccountId>,
    item: &NotificationItem,
) -> Result<&'a Arc<AdyenAccount>, PaymentError> {
    if let Some(id) = requested {
        return accounts.resolve(Some(id));
    }
    if accounts.len() == 1 {
        return accounts.resolve(None);
    }
    let merchant_account_code = item
        .merchant_account_code
        .as_deref()
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .ok_or_else(|| {
            PaymentError::validation(
                "adyen notification has no merchantAccountCode but several accounts are configured",
            )
        })?;
    let mut matched = None;
    for id in accounts.ids() {
        let account = accounts.resolve(Some(id))?;
        if account.merchant_account == merchant_account_code {
            if matched.is_some() {
                return Err(PaymentError::config(
                    PROVIDER,
                    format!(
                        "several configured adyen accounts use merchant account `{merchant_account_code}`"
                    ),
                ));
            }
            matched = Some(account);
        }
    }
    matched.ok_or_else(|| {
        // The merchantAccountCode is untrusted input at this point (the item is
        // not verified yet), so this is a caller error, not a server config one.
        PaymentError::validation(format!(
            "no configured adyen account matches the notification merchantAccountCode `{merchant_account_code}`"
        ))
    })
}

fn live_flag(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(value) => Some(*value),
        Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Boolish {
    Bool(bool),
    String(String),
}

fn de_option_string_boolish<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Boolish>::deserialize(deserializer)?;
    Ok(value.map(|value| match value {
        Boolish::Bool(value) => value.to_string(),
        Boolish::String(value) => value,
    }))
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Integerish {
    Integer(i64),
    String(String),
}

fn de_option_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Integerish>::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(Integerish::Integer(value)) => Ok(Some(value)),
        Some(Integerish::String(value)) => value
            .trim()
            .parse::<i64>()
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

/// Parse an RFC 3339 timestamp into Unix seconds (fractional seconds ignored).
pub(crate) fn parse_rfc3339_utc_seconds(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    if bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || !matches!(bytes.get(10), Some(b'T' | b't'))
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return None;
    }
    let year: i64 = text.get(0..4)?.parse().ok()?;
    let month: i64 = text.get(5..7)?.parse().ok()?;
    let day: i64 = text.get(8..10)?.parse().ok()?;
    let hour: i64 = text.get(11..13)?.parse().ok()?;
    let minute: i64 = text.get(14..16)?.parse().ok()?;
    let second: i64 = text.get(17..19)?.parse().ok()?;
    let mut index = 19;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        while matches!(bytes.get(index), Some(byte) if byte.is_ascii_digit()) {
            index += 1;
        }
    }
    let offset_seconds = match bytes.get(index) {
        Some(b'Z' | b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let sign = if *sign == b'-' { -1 } else { 1 };
            let rest = text.get(index + 1..)?;
            let (hours, minutes) = rest.split_once(':')?;
            let hours: i64 = hours.parse().ok()?;
            let minutes: i64 = minutes.parse().ok()?;
            sign * (hours * 3600 + minutes * 60)
        }
        Some(_) => return None,
        None => 0,
    };
    let days = days_from_civil(year, month, day)?;
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(hour * 3600 + minute * 60 + second)?;
    seconds.checked_sub(offset_seconds)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = (month + 9) % 12;
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::AdyenAccountOptions;
    use crate::options::AdyenOptions;
    use serde_json::json;

    const HMAC_FIXTURE: &str = "00112233445566778899aabbccddeeff";

    fn hmac_key() -> Vec<u8> {
        hmac::decode_hmac_key(HMAC_FIXTURE).unwrap()
    }

    fn item(psp_reference: &str, event_code: &str, success: &str) -> NotificationItem {
        serde_json::from_value(json!({
            "pspReference": psp_reference,
            "merchantAccountCode": "FixtureMerchant",
            "merchantReference": "ORDER-1",
            "amount": { "value": 9900, "currency": "EUR" },
            "eventCode": event_code,
            "success": success,
            "eventDate": "2020-01-01T00:00:00Z"
        }))
        .unwrap()
    }

    fn signed_item_json(psp_reference: &str, event_code: &str, success: &str) -> Value {
        let mut value = json!({
            "pspReference": psp_reference,
            "merchantAccountCode": "FixtureMerchant",
            "merchantReference": "ORDER-1",
            "amount": { "value": 9900, "currency": "EUR" },
            "eventCode": event_code,
            "success": success,
            "eventDate": "2020-01-01T00:00:00Z"
        });
        let parsed: NotificationItem = serde_json::from_value(value.clone()).unwrap();
        let signature = parsed.signature(&hmac_key());
        value["additionalData"] = json!({ "hmacSignature": signature });
        value
    }

    fn notification(items: Vec<Value>, live: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "live": live,
            "notificationItems": items
                .into_iter()
                .map(|item| json!({ "NotificationRequestItem": item }))
                .collect::<Vec<_>>()
        }))
        .unwrap()
    }

    fn verifier(options: AdyenOptions) -> AdyenWebhookVerifier {
        AdyenWebhookVerifier::new(Ok(options.build_accounts().unwrap()))
    }

    fn test_options() -> AdyenOptions {
        AdyenOptions::new("fixture-api-key", "FixtureMerchant", HMAC_FIXTURE)
    }

    #[tokio::test]
    async fn verifies_and_normalizes_a_batch_of_two_items() {
        let body = notification(
            vec![
                signed_item_json("7914073381342284", "AUTHORISATION", "true"),
                signed_item_json("JDD6LKT8MBLZNN84", "CAPTURE", "true"),
            ],
            "false",
        );
        let payload = WebhookPayload::new(Vec::<(String, String)>::new(), body);
        let events = verifier(test_options())
            .verify_all(None, &payload)
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, PaymentEventType::Succeeded);
        assert_eq!(events[0].dedupe_key, "7914073381342284:AUTHORISATION:true");
        assert_eq!(events[0].order_id.as_deref(), Some("ORDER-1"));
        assert_eq!(
            events[0].provider_payment_id.as_deref(),
            Some("7914073381342284")
        );
        assert_eq!(events[0].amount.unwrap().minor, 9900);
        assert_eq!(events[0].livemode, Some(false));
        assert_eq!(events[0].occurred_at, 1_577_836_800);
        assert_eq!(events[0].account, Some(AccountId::default_account()));
        assert_eq!(events[1].event_type, PaymentEventType::Succeeded);
        assert_eq!(events[1].dedupe_key, "JDD6LKT8MBLZNN84:CAPTURE:true");
    }

    #[tokio::test]
    async fn a_single_tampered_item_rejects_the_whole_delivery() {
        let mut tampered = signed_item_json("JDD6LKT8MBLZNN84", "CAPTURE", "true");
        tampered["amount"]["value"] = json!(1);
        let body = notification(
            vec![
                signed_item_json("7914073381342284", "AUTHORISATION", "true"),
                tampered,
            ],
            "false",
        );
        let payload = WebhookPayload::new(Vec::<(String, String)>::new(), body);
        let error = verifier(test_options())
            .verify_all(None, &payload)
            .await
            .unwrap_err();
        assert_eq!(error.public_detail(), Some("signature mismatch"));
        assert_eq!(error.http_status(), 400);
    }

    #[tokio::test]
    async fn a_missing_signature_fails_closed() {
        let body = notification(
            vec![json!({
                "pspReference": "7914073381342284",
                "merchantAccountCode": "FixtureMerchant",
                "merchantReference": "ORDER-1",
                "amount": { "value": 9900, "currency": "EUR" },
                "eventCode": "AUTHORISATION",
                "success": "true"
            })],
            "false",
        );
        let payload = WebhookPayload::new(Vec::<(String, String)>::new(), body);
        let error = verifier(test_options())
            .verify_all(None, &payload)
            .await
            .unwrap_err();
        assert_eq!(error.public_detail(), Some("missing HMAC signature"));
    }

    #[tokio::test]
    async fn single_event_deliveries_work_through_the_shared_port() {
        let body = notification(
            vec![signed_item_json(
                "7914073381342284",
                "AUTHORISATION",
                "false",
            )],
            "true",
        );
        let payload = WebhookPayload::new(Vec::<(String, String)>::new(), body);
        let verifier = verifier(test_options());
        let event =
            IPaymentWebhookVerifier::verify(&verifier, &PaymentScope::new(PROVIDER), &payload)
                .await
                .unwrap();
        assert_eq!(event.event_type, PaymentEventType::Failed);
        assert_eq!(event.livemode, Some(true));
        assert_eq!(verifier.provider(), ProviderId::Adyen);
    }

    #[test]
    fn signing_string_uses_the_documented_order_and_escaping() {
        let mut value = json!({
            "pspReference": "A:B",
            "originalReference": "C\\D",
            "merchantAccountCode": "FixtureMerchant",
            "merchantReference": "ORDER:1\\2",
            "amount": { "value": 9900, "currency": "EUR" },
            "eventCode": "AUTHORISATION",
            "success": "true"
        });
        let parsed: NotificationItem = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            parsed.signing_string(),
            "A\\:B:C\\\\D:FixtureMerchant:ORDER\\:1\\\\2:9900:EUR:AUTHORISATION:true"
        );
        // Round-trip: the same item verifies against a signature we produce.
        let signature = parsed.signature(&hmac_key());
        assert!(parsed.verify_signature(&hmac_key(), &signature).is_ok());
        value["pspReference"] = json!("A:C");
        let tampered: NotificationItem = serde_json::from_value(value).unwrap();
        assert!(tampered.verify_signature(&hmac_key(), &signature).is_err());
    }

    #[test]
    fn normalizes_every_documented_event_code() {
        let cases = [
            ("AUTHORISATION", "true", PaymentEventType::Succeeded),
            ("AUTHORISATION", "false", PaymentEventType::Failed),
            ("CAPTURE", "true", PaymentEventType::Succeeded),
            ("CAPTURE", "false", PaymentEventType::Failed),
            ("REFUND", "true", PaymentEventType::RefundSucceeded),
            ("REFUND", "false", PaymentEventType::RefundFailed),
            ("REFUND_FAILED", "true", PaymentEventType::RefundFailed),
            ("CANCELLATION", "true", PaymentEventType::Canceled),
            (
                "CANCELLATION",
                "false",
                PaymentEventType::Unknown("CANCELLATION_FAILED".to_string()),
            ),
            ("CHARGEBACK", "true", PaymentEventType::DisputeOpened),
            ("DISPUTE", "true", PaymentEventType::DisputeOpened),
            (
                "NOTIFICATION_OF_CHARGEBACK",
                "true",
                PaymentEventType::DisputeOpened,
            ),
        ];
        for (event_code, success, expected) in cases {
            let event = normalize_event(
                &item("7914073381342284", event_code, success),
                AccountId::default_account(),
                Some(false),
                Value::Null,
            );
            assert_eq!(event.event_type, expected, "{event_code}/{success}");
        }
        let unknown = normalize_event(
            &item("7914073381342284", "SOMETHING_NEW", "true"),
            AccountId::default_account(),
            None,
            Value::Null,
        );
        assert_eq!(
            unknown.event_type,
            PaymentEventType::Unknown("SOMETHING_NEW".to_string())
        );
    }

    #[test]
    fn capture_events_reference_the_original_payment() {
        let value = json!({
            "pspReference": "JDD6LKT8MBLZNN84",
            "originalReference": "7914073381342284",
            "merchantAccountCode": "FixtureMerchant",
            "eventCode": "CAPTURE",
            "success": "true"
        });
        let parsed: NotificationItem = serde_json::from_value(value).unwrap();
        let event = normalize_event(
            &parsed,
            AccountId::default_account(),
            Some(false),
            Value::Null,
        );
        assert_eq!(
            event.provider_payment_id.as_deref(),
            Some("7914073381342284")
        );
        assert_eq!(event.amount, None);
    }

    #[test]
    fn amount_values_accept_numbers_and_numeric_strings() {
        let numeric: NotificationItem = serde_json::from_value(json!({
            "pspReference": "7914073381342284",
            "amount": { "value": 1130, "currency": "EUR" }
        }))
        .unwrap();
        assert_eq!(numeric.amount.as_ref().unwrap().value, Some(1130));
        let text: NotificationItem = serde_json::from_value(json!({
            "pspReference": "7914073381342284",
            "amount": { "value": "1130", "currency": "EUR" },
            "success": true
        }))
        .unwrap();
        assert_eq!(text.amount.as_ref().unwrap().value, Some(1130));
        assert_eq!(text.success.as_deref(), Some("true"));
        assert!(text.success_bool());
    }

    #[test]
    fn live_flag_accepts_strings_and_booleans() {
        let body = notification(
            vec![signed_item_json(
                "7914073381342284",
                "AUTHORISATION",
                "true",
            )],
            "false",
        );
        assert_eq!(parse_notification(&body).unwrap().live, Some(false));
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(live_flag(value.get("live")), Some(false));
        assert_eq!(live_flag(Some(&json!("true"))), Some(true));
        assert_eq!(live_flag(Some(&json!(true))), Some(true));
        assert_eq!(live_flag(Some(&json!("maybe"))), None);
        assert_eq!(live_flag(None), None);
    }

    #[test]
    fn rejects_malformed_batches() {
        assert!(parse_notification(b"not json").is_err());
        assert!(parse_notification(br#"{"live":"false"}"#).is_err());
        assert!(parse_notification(br#"{"notificationItems":[]}"#).is_err());
        assert!(parse_notification(br#"{"notificationItems":[{}]}"#).is_err());
    }

    #[tokio::test]
    async fn routes_batched_items_to_the_matching_account() {
        let options = AdyenOptions::new("fixture-api-key", "FixtureMerchant", HMAC_FIXTURE)
            .with_account(
                AdyenAccountOptions::new("fixture-api-key-2", "FixtureMerchant2", HMAC_FIXTURE)
                    .with_id("secondary"),
            )
            .with_default_account("default");
        let verifier = verifier(options);
        let payload_for = |merchant_code: &str| {
            let mut value = json!({
                "pspReference": "7914073381342284",
                "merchantAccountCode": merchant_code,
                "merchantReference": "ORDER-1",
                "amount": { "value": 9900, "currency": "EUR" },
                "eventCode": "AUTHORISATION",
                "success": "true"
            });
            let parsed: NotificationItem = serde_json::from_value(value.clone()).unwrap();
            value["additionalData"] = json!({ "hmacSignature": parsed.signature(&hmac_key()) });
            WebhookPayload::new(
                Vec::<(String, String)>::new(),
                notification(vec![value], "false"),
            )
        };

        let events = verifier
            .verify_all(None, &payload_for("FixtureMerchant2"))
            .await
            .unwrap();
        assert_eq!(events[0].account.as_ref().unwrap().as_str(), "secondary");

        let events = verifier
            .verify_all(None, &payload_for("FixtureMerchant"))
            .await
            .unwrap();
        assert_eq!(events[0].account.as_ref().unwrap().as_str(), "default");

        let error = verifier
            .verify_all(None, &payload_for("UnknownMerchant"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("UnknownMerchant"));
    }

    #[test]
    fn parses_rfc3339_timestamps() {
        assert_eq!(
            parse_rfc3339_utc_seconds("2020-01-01T00:00:00Z"),
            Some(1_577_836_800)
        );
        assert_eq!(
            parse_rfc3339_utc_seconds("2020-01-01T02:00:00+02:00"),
            Some(1_577_836_800)
        );
        assert_eq!(
            parse_rfc3339_utc_seconds("2019-05-06T17:15:34.121+02:00"),
            Some(1_557_155_734)
        );
        assert_eq!(parse_rfc3339_utc_seconds("not-a-date"), None);
        assert_eq!(parse_rfc3339_utc_seconds("2020-13-01T00:00:00Z"), None);
    }

    #[test]
    fn debug_of_parsed_items_does_not_include_the_signature_source() {
        let item = signed_item_json("7914073381342284", "AUTHORISATION", "true");
        let parsed: NotificationItem = serde_json::from_value(item).unwrap();
        let text = format!("{parsed:?}");
        assert!(text.contains("7914073381342284"));
        assert!(!text.contains(HMAC_FIXTURE));
    }
}
