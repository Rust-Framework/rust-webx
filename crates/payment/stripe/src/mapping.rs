//! Stripe object → shared model mapping.

use serde_json::Value;
use webx_pay::{Currency, Money, PaymentStatus, RefundStatus};

/// Map a PaymentIntent status to the shared state.
pub fn map_status(status: &str) -> PaymentStatus {
    match status {
        "requires_payment_method" | "requires_confirmation" | "requires_action" => {
            PaymentStatus::RequiresAction
        }
        "processing" => PaymentStatus::Processing,
        "requires_capture" => PaymentStatus::Authorized,
        "succeeded" => PaymentStatus::Succeeded,
        "canceled" => PaymentStatus::Canceled,
        _ => PaymentStatus::Unknown,
    }
}

/// Map a Refund status to the shared state.
pub fn map_refund_status(status: &str) -> RefundStatus {
    match status {
        "pending" => RefundStatus::Pending,
        "succeeded" => RefundStatus::Succeeded,
        "failed" => RefundStatus::Failed,
        "canceled" => RefundStatus::Canceled,
        _ => RefundStatus::Unknown,
    }
}

/// Map a Checkout Session `(status, payment_status)` pair.
pub fn map_checkout_status(status: &str, payment_status: &str) -> PaymentStatus {
    match (status, payment_status) {
        ("complete", "paid") | ("complete", "no_payment_required") => PaymentStatus::Succeeded,
        ("open", _) => PaymentStatus::RequiresAction,
        ("expired", _) => PaymentStatus::Canceled,
        ("complete", "unpaid") => PaymentStatus::Processing,
        _ => PaymentStatus::Unknown,
    }
}

/// Build `Money` from Stripe's integer `amount` + ISO currency string.
pub fn money_from(amount: Option<i64>, currency: Option<&str>) -> Option<Money> {
    let amount = amount?;
    let currency = Currency::new(currency?).ok()?;
    Some(Money::new(currency, amount))
}

/// Read a string field.
pub fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// Read an i64 field.
pub fn int_field(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

/// Read `metadata.order_id` (or `client_reference_id`) from an object.
pub fn order_id_from(value: &Value) -> Option<String> {
    value
        .get("metadata")
        .and_then(|metadata| metadata.get("order_id"))
        .and_then(Value::as_str)
        .or_else(|| string_field(value, "client_reference_id"))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_payment_intent_states() {
        assert_eq!(map_status("succeeded"), PaymentStatus::Succeeded);
        assert_eq!(map_status("requires_capture"), PaymentStatus::Authorized);
        assert_eq!(map_status("processing"), PaymentStatus::Processing);
        assert_eq!(map_status("canceled"), PaymentStatus::Canceled);
        assert_eq!(map_status("new_state"), PaymentStatus::Unknown);
    }

    #[test]
    fn maps_checkout_sessions() {
        assert_eq!(
            map_checkout_status("complete", "paid"),
            PaymentStatus::Succeeded
        );
        assert_eq!(
            map_checkout_status("open", "unpaid"),
            PaymentStatus::RequiresAction
        );
        assert_eq!(
            map_checkout_status("expired", "unpaid"),
            PaymentStatus::Canceled
        );
    }

    #[test]
    fn builds_money_and_ignores_invalid_currency() {
        assert_eq!(
            money_from(Some(9900), Some("cny")),
            Some(Money::new(Currency::CNY, 9900))
        );
        assert_eq!(money_from(Some(9900), Some("us")), None);
        assert_eq!(money_from(None, Some("cny")), None);
    }
}
