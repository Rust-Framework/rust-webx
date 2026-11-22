//! Alipay wire values → shared payment model.
//!
//! Mapping is deliberately small and fail-closed: an unknown status becomes
//! [`PaymentStatus::Unknown`] / [`PaymentEventType::Unknown`] instead of being
//! guessed into `Succeeded`.

use serde_json::Value;
use webx_pay::{Currency, Money, PaymentError, PaymentEventType, PaymentStatus, RefundStatus};

/// Map an Alipay `trade_status` to the shared payment state.
pub fn map_trade_status(status: &str) -> PaymentStatus {
    match status {
        "WAIT_BUYER_PAY" => PaymentStatus::RequiresAction,
        "TRADE_SUCCESS" | "TRADE_FINISHED" => PaymentStatus::Succeeded,
        "TRADE_CLOSED" => PaymentStatus::Canceled,
        _ => PaymentStatus::Unknown,
    }
}

/// Map the `refund_status` field of `alipay.trade.fastpay.refund.query`.
///
/// Official semantics: only `REFUND_SUCCESS` is a positive status; an absent
/// field means the refund was not received (or not processed) yet, so it is
/// reported as [`RefundStatus::Pending`] and must be queried again later.
pub fn map_refund_status(status: Option<&str>) -> RefundStatus {
    match status {
        Some("REFUND_SUCCESS") => RefundStatus::Succeeded,
        Some(_) => RefundStatus::Unknown,
        None => RefundStatus::Pending,
    }
}

/// Map a notification `trade_status` to the shared event type.
pub(crate) fn event_type_for_trade_status(status: &str) -> PaymentEventType {
    match status {
        "TRADE_SUCCESS" | "TRADE_FINISHED" => PaymentEventType::Succeeded,
        "TRADE_CLOSED" => PaymentEventType::Canceled,
        other => PaymentEventType::Unknown(other.to_string()),
    }
}

/// Parse an Alipay yuan amount string such as `"99.00"` into [`Money`].
pub(crate) fn money_cny(value: &str) -> Option<Money> {
    Money::from_major_str(Currency::CNY, value).ok()
}

/// Render [`Money`] as Alipay's two-decimal yuan string.
pub(crate) fn cny_amount_string(amount: &Money) -> Result<String, PaymentError> {
    if amount.currency != Currency::CNY {
        return Err(PaymentError::validation(format!(
            "alipay supports CNY amounts only (got {})",
            amount.currency
        )));
    }
    Ok(amount.to_major_string())
}

/// Read a string field. Alipay responses occasionally encode ids as numbers, so
/// numeric JSON values are rendered as their decimal string form.
pub(crate) fn string_field(value: &Value, key: &str) -> Option<String> {
    match value.get(key)? {
        Value::String(item) => Some(item.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_trade_states() {
        assert_eq!(
            map_trade_status("WAIT_BUYER_PAY"),
            PaymentStatus::RequiresAction
        );
        assert_eq!(map_trade_status("TRADE_SUCCESS"), PaymentStatus::Succeeded);
        assert_eq!(map_trade_status("TRADE_FINISHED"), PaymentStatus::Succeeded);
        assert_eq!(map_trade_status("TRADE_CLOSED"), PaymentStatus::Canceled);
        assert_eq!(map_trade_status("WHATEVER"), PaymentStatus::Unknown);
    }

    #[test]
    fn maps_refund_query_states() {
        assert_eq!(
            map_refund_status(Some("REFUND_SUCCESS")),
            RefundStatus::Succeeded
        );
        assert_eq!(map_refund_status(None), RefundStatus::Pending);
        assert_eq!(map_refund_status(Some("SOMETHING")), RefundStatus::Unknown);
    }

    #[test]
    fn maps_notification_event_types() {
        assert_eq!(
            event_type_for_trade_status("TRADE_SUCCESS"),
            PaymentEventType::Succeeded
        );
        assert_eq!(
            event_type_for_trade_status("TRADE_CLOSED"),
            PaymentEventType::Canceled
        );
        assert_eq!(
            event_type_for_trade_status("WAIT_BUYER_PAY"),
            PaymentEventType::Unknown("WAIT_BUYER_PAY".to_string())
        );
    }

    #[test]
    fn parses_and_renders_yuan_strings() {
        assert_eq!(money_cny("99.00"), Some(Money::cny(9900)));
        assert_eq!(money_cny("0.01"), Some(Money::cny(1)));
        assert_eq!(money_cny("1.234"), None);
        assert_eq!(money_cny("abc"), None);
        assert_eq!(cny_amount_string(&Money::cny(9900)).unwrap(), "99.00");
        assert!(cny_amount_string(&Money::usd(100)).is_err());
    }

    #[test]
    fn string_field_accepts_numbers() {
        let value = serde_json::json!({"trade_no": 2023111522001i64, "name": "x"});
        assert_eq!(
            string_field(&value, "trade_no").as_deref(),
            Some("2023111522001")
        );
        assert_eq!(string_field(&value, "name").as_deref(), Some("x"));
        assert_eq!(string_field(&value, "missing"), None);
    }
}
