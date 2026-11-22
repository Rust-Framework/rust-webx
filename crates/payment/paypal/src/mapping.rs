//! PayPal status/JSON → shared model mapping.
//!
//! PayPal speaks decimal-string money (`{"currency_code": "USD",
//! "value": "99.00"}`), RFC 3339 timestamps and provider-specific status
//! enums. The shared [`webx_pay`] model uses integer minor units, Unix seconds
//! and normalized states, so every conversion lives here and nowhere else.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use webx_pay::{Currency, Money, PaymentStatus, RefundStatus};

/// Map a PayPal Orders v2 order status to the shared lifecycle state.
///
/// `intent` is the order's `CAPTURE`/`AUTHORIZE` intent. An `APPROVED` order
/// needs a capture when the intent is `CAPTURE`; with `AUTHORIZE` the payer
/// approval already produced an authorization and funds are captured later.
pub fn map_order_status(status: &str, intent: Option<&str>) -> PaymentStatus {
    match status {
        "CREATED" | "SAVED" => PaymentStatus::Created,
        "APPROVED" => match intent {
            Some("AUTHORIZE") => PaymentStatus::Authorized,
            _ => PaymentStatus::RequiresAction,
        },
        "COMPLETED" => PaymentStatus::Succeeded,
        "VOIDED" => PaymentStatus::Canceled,
        "PAYER_ACTION_REQUIRED" => PaymentStatus::RequiresAction,
        _ => PaymentStatus::Unknown,
    }
}

/// Map a PayPal capture status to the shared lifecycle state.
pub fn map_capture_status(status: &str) -> PaymentStatus {
    match status {
        "PENDING" => PaymentStatus::Processing,
        "COMPLETED" => PaymentStatus::Succeeded,
        "DECLINED" | "FAILED" => PaymentStatus::Failed,
        "REFUNDED" => PaymentStatus::Refunded,
        "PARTIALLY_REFUNDED" => PaymentStatus::PartiallyRefunded,
        _ => PaymentStatus::Unknown,
    }
}

/// Map a PayPal refund status to the shared refund state.
pub fn map_refund_status(status: &str) -> RefundStatus {
    match status {
        "PENDING" => RefundStatus::Pending,
        "COMPLETED" => RefundStatus::Succeeded,
        "CANCELLED" => RefundStatus::Canceled,
        "FAILED" => RefundStatus::Failed,
        _ => RefundStatus::Unknown,
    }
}

/// Parse a PayPal decimal-string amount (`{"currency_code", "value"}`) into
/// minor units.
///
/// Legacy Payments v1 (`{"total": "7.47", "currency": "USD"}`) shapes are also
/// accepted because `PAYMENT.SALE.*` webhooks still use them.
pub(crate) fn money_from_object(value: &Value) -> Option<Money> {
    if let (Some(code), Some(amount)) = (
        value.get("currency_code").and_then(Value::as_str),
        value.get("value").and_then(Value::as_str),
    ) {
        return money_from_str(code, amount);
    }
    if let (Some(amount), Some(code)) = (
        value.get("total").and_then(Value::as_str),
        value.get("currency").and_then(Value::as_str),
    ) {
        return money_from_str(code, amount);
    }
    None
}

fn money_from_str(code: &str, amount: &str) -> Option<Money> {
    let currency = Currency::new(code).ok()?;
    Money::from_major_str(currency, amount).ok()
}

/// Extract an amount from a webhook `resource` object.
///
/// Looks at `resource.amount` first (Payments v2 captures/refunds and legacy
/// v1 sales), then at the first purchase unit's amount (Orders v2 order
/// resources emitted by `CHECKOUT.ORDER.*` events).
pub(crate) fn money_from_resource(resource: &Value) -> Option<Money> {
    if let Some(amount) = resource.get("amount").and_then(money_from_object) {
        return Some(amount);
    }
    resource
        .get("purchase_units")
        .and_then(Value::as_array)
        .and_then(|units| units.first())
        .and_then(|unit| unit.get("amount"))
        .and_then(money_from_object)
}

/// Current Unix time in seconds (the fallback for missing provider timestamps).
pub(crate) fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Parse an RFC 3339 timestamp (`2024-10-14T21:29:26Z`,
/// `2024-10-14T14:58:34-07:00`, optional fractional seconds) to Unix seconds.
///
/// Payment payloads only carry UTC or fixed-offset timestamps; leap seconds are
/// clamped to the normal range instead of being rejected.
pub(crate) fn parse_rfc3339_secs(value: &str) -> Option<i64> {
    let value = value.trim();
    let (date, rest) = value.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let (time, offset_seconds) = split_offset(rest)?;
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let seconds = time_parts.next()?;
    if time_parts.next().is_some() || !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
        return None;
    }
    // `2024-10-14T21:29:26.123Z`: keep whole seconds only.
    let seconds: i64 = seconds.split('.').next()?.parse().ok()?;
    if !(0..=60).contains(&seconds) {
        return None;
    }

    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + seconds.min(59) - offset_seconds)
}

fn split_offset(rest: &str) -> Option<(&str, i64)> {
    if let Some(time) = rest.strip_suffix('Z') {
        return Some((time, 0));
    }
    let (time, offset) = rest.split_at(rest.rfind(['+', '-'])?);
    let sign = if offset.starts_with('-') { -1 } else { 1 };
    let (hours, minutes) = offset[1..].split_once(':')?;
    let hours: i64 = hours.parse().ok()?;
    let minutes: i64 = minutes.parse().ok()?;
    Some((time, sign * (hours * 3_600 + minutes * 60)))
}

/// Days since 1970-01-01 for a proleptic Gregorian calendar date (Howard
/// Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_order_states_for_both_intents() {
        assert_eq!(
            map_order_status("CREATED", Some("CAPTURE")),
            PaymentStatus::Created
        );
        assert_eq!(
            map_order_status("SAVED", Some("CAPTURE")),
            PaymentStatus::Created
        );
        assert_eq!(
            map_order_status("APPROVED", Some("CAPTURE")),
            PaymentStatus::RequiresAction
        );
        assert_eq!(
            map_order_status("APPROVED", Some("AUTHORIZE")),
            PaymentStatus::Authorized
        );
        assert_eq!(
            map_order_status("COMPLETED", None),
            PaymentStatus::Succeeded
        );
        assert_eq!(map_order_status("VOIDED", None), PaymentStatus::Canceled);
        assert_eq!(
            map_order_status("PAYER_ACTION_REQUIRED", None),
            PaymentStatus::RequiresAction
        );
        assert_eq!(
            map_order_status("SOMETHING_NEW", None),
            PaymentStatus::Unknown
        );
    }

    #[test]
    fn maps_capture_states() {
        assert_eq!(map_capture_status("PENDING"), PaymentStatus::Processing);
        assert_eq!(map_capture_status("COMPLETED"), PaymentStatus::Succeeded);
        assert_eq!(map_capture_status("DECLINED"), PaymentStatus::Failed);
        assert_eq!(map_capture_status("FAILED"), PaymentStatus::Failed);
        assert_eq!(map_capture_status("REFUNDED"), PaymentStatus::Refunded);
        assert_eq!(
            map_capture_status("PARTIALLY_REFUNDED"),
            PaymentStatus::PartiallyRefunded
        );
        assert_eq!(map_capture_status("NEW_STATE"), PaymentStatus::Unknown);
    }

    #[test]
    fn maps_refund_states() {
        assert_eq!(map_refund_status("PENDING"), RefundStatus::Pending);
        assert_eq!(map_refund_status("COMPLETED"), RefundStatus::Succeeded);
        assert_eq!(map_refund_status("CANCELLED"), RefundStatus::Canceled);
        assert_eq!(map_refund_status("FAILED"), RefundStatus::Failed);
        assert_eq!(map_refund_status("OTHER"), RefundStatus::Unknown);
    }

    #[test]
    fn parses_rfc3339_utc_and_fixed_offsets() {
        assert_eq!(
            parse_rfc3339_secs("2024-10-14T21:29:26Z"),
            Some(1_728_941_366)
        );
        assert_eq!(
            parse_rfc3339_secs("2018-04-01T21:20:49Z"),
            Some(1_522_617_649)
        );
        assert_eq!(
            parse_rfc3339_secs("2024-10-14T14:58:34-07:00"),
            Some(1_728_943_114)
        );
        assert_eq!(
            parse_rfc3339_secs("2024-10-14T21:29:26.987Z"),
            Some(1_728_941_366)
        );
        assert_eq!(parse_rfc3339_secs("not-a-date"), None);
        assert_eq!(parse_rfc3339_secs("2024-13-01T00:00:00Z"), None);
    }

    #[test]
    fn parses_v2_and_v1_amount_shapes() {
        let v2 = serde_json::json!({ "currency_code": "USD", "value": "99.00" });
        assert_eq!(
            money_from_object(&v2),
            Some(Money::new(Currency::USD, 9900))
        );

        let v1 = serde_json::json!({ "total": "7.47", "currency": "USD" });
        assert_eq!(money_from_object(&v1), Some(Money::new(Currency::USD, 747)));

        let order = serde_json::json!({
            "purchase_units": [{ "amount": { "currency_code": "JPY", "value": "1000" } }]
        });
        assert_eq!(
            money_from_resource(&order),
            Some(Money::new(Currency::JPY, 1000))
        );
        assert_eq!(money_from_object(&serde_json::json!({})), None);
    }
}
