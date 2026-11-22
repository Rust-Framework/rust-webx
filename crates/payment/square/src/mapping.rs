//! Square objects → shared payment model mapping.
//!
//! Square speaks integer minor units (`Money { amount, currency }`) and string
//! statuses; this module is the single place where those become the
//! provider-neutral [`PaymentStatus`], [`RefundStatus`], [`Money`] and
//! [`PaymentEventType`] values. Unknown Square values map to `Unknown` instead
//! of being guessed into a success state.

use serde_json::{json, Value};
use webx_pay::{Currency, Money, PaymentEventType, PaymentStatus, RefundStatus};

/// Map a Square `Payment.status` to the shared state.
///
/// Square documents `APPROVED`, `PENDING`, `COMPLETED`, `CANCELED` and `FAILED`.
pub fn map_payment_status(status: &str) -> PaymentStatus {
    let status = status.trim().to_ascii_uppercase();
    match status.as_str() {
        "APPROVED" => PaymentStatus::Authorized,
        "COMPLETED" => PaymentStatus::Succeeded,
        "CANCELED" | "CANCELLED" => PaymentStatus::Canceled,
        "FAILED" => PaymentStatus::Failed,
        "PENDING" => PaymentStatus::Processing,
        _ => PaymentStatus::Unknown,
    }
}

/// Map a Square `PaymentRefund.status` to the shared state.
///
/// The documented refund statuses are `PENDING`, `APPROVED`, `REJECTED` and
/// `FAILED`; `COMPLETED` is accepted as an alias of `APPROVED` because some
/// webhook payloads and older responses report it.
pub fn map_refund_status(status: &str) -> RefundStatus {
    let status = status.trim().to_ascii_uppercase();
    match status.as_str() {
        "PENDING" => RefundStatus::Pending,
        "APPROVED" | "COMPLETED" => RefundStatus::Succeeded,
        "FAILED" | "REJECTED" => RefundStatus::Failed,
        "CANCELED" | "CANCELLED" => RefundStatus::Canceled,
        _ => RefundStatus::Unknown,
    }
}

/// Normalize a `payment.*` webhook event from the payment status it carries.
pub fn map_payment_event_type(kind: &str, status: &str) -> PaymentEventType {
    match map_payment_status(status) {
        PaymentStatus::Succeeded => PaymentEventType::Succeeded,
        PaymentStatus::Authorized => PaymentEventType::AuthorizationSucceeded,
        PaymentStatus::Canceled => PaymentEventType::Canceled,
        PaymentStatus::Failed => PaymentEventType::Failed,
        _ => PaymentEventType::Unknown(kind.to_string()),
    }
}

/// Normalize a `refund.*` webhook event from the refund status it carries.
pub fn map_refund_event_type(kind: &str, status: &str) -> PaymentEventType {
    match map_refund_status(status) {
        RefundStatus::Succeeded => PaymentEventType::RefundSucceeded,
        RefundStatus::Failed | RefundStatus::Canceled => PaymentEventType::RefundFailed,
        _ => PaymentEventType::Unknown(kind.to_string()),
    }
}

/// Build [`Money`] from a Square `Money` object (`{ "amount": 9900, "currency": "USD" }`).
pub fn money_from(value: Option<&Value>) -> Option<Money> {
    let value = value?;
    let amount = value.get("amount").and_then(Value::as_i64)?;
    let currency = value.get("currency").and_then(Value::as_str)?;
    let currency = Currency::new(currency).ok()?;
    Some(Money::new(currency, amount))
}

/// Render [`Money`] as the Square `Money` object used in request bodies.
pub fn money_json(money: &Money) -> Value {
    json!({
        "amount": money.minor,
        "currency": money.currency.code().to_ascii_uppercase(),
    })
}

/// Read a string field from a JSON object.
pub fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// Parse an RFC 3339 timestamp (`2020-02-06T21:27:34.308Z`) into Unix seconds.
///
/// Square returns timestamps with millisecond precision and a `Z` suffix; an
/// explicit numeric offset (`+02:00`) is also accepted. Fractional seconds are
/// truncated. `None` is returned for malformed input instead of panicking.
pub fn parse_timestamp(value: &str) -> Option<i64> {
    let value = value.trim();
    let bytes = value.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let digits = |range: std::ops::Range<usize>| value.get(range)?.parse::<i64>().ok();
    let year = digits(0..4)?;
    if value.get(4..5)? != "-" {
        return None;
    }
    let month = digits(5..7)?;
    if value.get(7..8)? != "-" {
        return None;
    }
    let day = digits(8..10)?;
    let separator = value.get(10..11)?;
    if separator != "T" && separator != "t" && separator != " " {
        return None;
    }
    let hour = digits(11..13)?;
    if value.get(13..14)? != ":" {
        return None;
    }
    let minute = digits(14..16)?;
    if value.get(16..17)? != ":" {
        return None;
    }
    let second = digits(17..19)?;

    let mut rest = &value[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let fraction_digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if fraction_digits == 0 {
            return None;
        }
        rest = &fraction[fraction_digits..];
    }
    let offset_seconds = match rest {
        "" | "Z" | "z" => 0,
        _ => parse_offset(rest)?,
    };

    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }

    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second - offset_seconds)
}

fn parse_offset(value: &str) -> Option<i64> {
    let sign = match value.get(0..1)? {
        "+" => 1,
        "-" => -1,
        _ => return None,
    };
    let body = value.get(1..)?;
    let (hours, minutes) = match body.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        None if body.len() == 4 => (&body[0..2], &body[2..4]),
        None => return None,
    };
    if hours.len() != 2 || minutes.len() != 2 {
        return None;
    }
    let hours: i64 = hours.parse().ok()?;
    let minutes: i64 = minutes.parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 3_600 + minutes * 60))
}

/// Days since the Unix epoch for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Current Unix time in seconds.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_square_payment_statuses() {
        assert_eq!(map_payment_status("COMPLETED"), PaymentStatus::Succeeded);
        assert_eq!(map_payment_status("APPROVED"), PaymentStatus::Authorized);
        assert_eq!(map_payment_status("PENDING"), PaymentStatus::Processing);
        assert_eq!(map_payment_status("CANCELED"), PaymentStatus::Canceled);
        assert_eq!(map_payment_status("FAILED"), PaymentStatus::Failed);
        assert_eq!(map_payment_status("approved"), PaymentStatus::Authorized);
        assert_eq!(map_payment_status("SOMETHING_NEW"), PaymentStatus::Unknown);
    }

    #[test]
    fn maps_square_refund_statuses() {
        assert_eq!(map_refund_status("PENDING"), RefundStatus::Pending);
        assert_eq!(map_refund_status("APPROVED"), RefundStatus::Succeeded);
        assert_eq!(map_refund_status("COMPLETED"), RefundStatus::Succeeded);
        assert_eq!(map_refund_status("REJECTED"), RefundStatus::Failed);
        assert_eq!(map_refund_status("FAILED"), RefundStatus::Failed);
        assert_eq!(map_refund_status("CANCELED"), RefundStatus::Canceled);
        assert_eq!(map_refund_status("NEW"), RefundStatus::Unknown);
    }

    #[test]
    fn maps_webhook_event_types_through_statuses() {
        assert_eq!(
            map_payment_event_type("payment.updated", "COMPLETED"),
            PaymentEventType::Succeeded
        );
        assert_eq!(
            map_payment_event_type("payment.updated", "APPROVED"),
            PaymentEventType::AuthorizationSucceeded
        );
        assert_eq!(
            map_payment_event_type("payment.updated", "CANCELED"),
            PaymentEventType::Canceled
        );
        assert_eq!(
            map_payment_event_type("payment.updated", "FAILED"),
            PaymentEventType::Failed
        );
        assert_eq!(
            map_payment_event_type("payment.updated", "PENDING"),
            PaymentEventType::Unknown("payment.updated".to_string())
        );
        assert_eq!(
            map_refund_event_type("refund.updated", "COMPLETED"),
            PaymentEventType::RefundSucceeded
        );
        assert_eq!(
            map_refund_event_type("refund.updated", "REJECTED"),
            PaymentEventType::RefundFailed
        );
        assert_eq!(
            map_refund_event_type("refund.updated", "PENDING"),
            PaymentEventType::Unknown("refund.updated".to_string())
        );
    }

    #[test]
    fn builds_and_parses_minor_unit_money() {
        let value = json!({ "amount": 9900, "currency": "USD" });
        assert_eq!(
            money_from(Some(&value)),
            Some(Money::new(Currency::USD, 9900))
        );
        assert_eq!(money_from(None), None);
        assert_eq!(money_from(Some(&json!({ "amount": 1 }))), None);
        assert_eq!(
            money_from(Some(&json!({ "amount": 1, "currency": "us" }))),
            None
        );
        assert_eq!(
            money_json(&Money::new(Currency::JPY, 1000)),
            json!({ "amount": 1000, "currency": "JPY" })
        );
    }

    #[test]
    fn parses_rfc3339_timestamps() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_timestamp("2020-02-06T21:27:34.308Z"),
            Some(1_581_024_454)
        );
        assert_eq!(
            parse_timestamp("2020-02-06T21:27:34+02:00"),
            Some(1_581_024_454 - 7_200)
        );
        assert_eq!(
            parse_timestamp("2020-02-06T21:27:34.308"),
            Some(1_581_024_454)
        );
        assert_eq!(parse_timestamp("not-a-timestamp"), None);
        assert_eq!(parse_timestamp("2020-13-06T21:27:34Z"), None);
        assert_eq!(parse_timestamp("2020-02-06 21:27:34Z"), Some(1_581_024_454));
    }
}
