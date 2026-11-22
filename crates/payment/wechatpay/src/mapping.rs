//! WeChat Pay wire values → shared payment model mapping.
//!
//! Status names are the documented enumerations (`trade_state`,
//! `refund_status`); an unrecognized value maps to `Unknown` instead of being
//! guessed into a terminal state. All amounts in this adapter are integer fen
//! and the currency is fixed to CNY, so no decimal conversion happens anywhere.

use serde_json::Value;
use webx_pay::{Money, PaymentStatus, RefundStatus};

/// Map a `trade_state` value to the shared payment state.
///
/// `REFUND` means "转入退款": the order has a refund. The query API does not
/// report the refunded amount, so a partial refund also surfaces as
/// [`PaymentStatus::Refunded`]; the raw `trade_state` is preserved in
/// `PaymentIntent::provider_status` for callers that need to distinguish.
pub fn map_trade_state(trade_state: &str) -> PaymentStatus {
    match trade_state {
        "SUCCESS" => PaymentStatus::Succeeded,
        "REFUND" => PaymentStatus::Refunded,
        "NOTPAY" => PaymentStatus::RequiresAction,
        "CLOSED" | "REVOKED" => PaymentStatus::Canceled,
        "USERPAYING" => PaymentStatus::Processing,
        "PAYERROR" => PaymentStatus::Failed,
        _ => PaymentStatus::Unknown,
    }
}

/// Map a refund `status` value to the shared refund state.
///
/// `ABNORMAL` (退款异常, for example a closed bank card) is a terminal failure
/// that requires manual handling or the "异常退款" API; it maps to
/// [`RefundStatus::Failed`]. `CLOSED` means the refund was closed, which this
/// model represents as [`RefundStatus::Canceled`].
pub fn map_refund_status(status: &str) -> RefundStatus {
    match status {
        "SUCCESS" => RefundStatus::Succeeded,
        "PROCESSING" => RefundStatus::Pending,
        "ABNORMAL" => RefundStatus::Failed,
        "CLOSED" => RefundStatus::Canceled,
        _ => RefundStatus::Unknown,
    }
}

/// CNY money from an integer fen amount.
pub(crate) fn money_from_fen(fen: i64) -> Money {
    Money::cny(fen)
}

/// Read a string field.
pub(crate) fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// Read an integer field.
pub(crate) fn int_field(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

/// Read `amount.<key>` as integer fen.
pub(crate) fn amount_field(value: &Value, key: &str) -> Option<i64> {
    value
        .get("amount")
        .and_then(|amount| int_field(amount, key))
}

/// Read `amount.currency` (always CNY for this adapter, but preserved for the
/// request-side check in the provider).
pub(crate) fn amount_currency(value: &Value) -> Option<&str> {
    value
        .get("amount")
        .and_then(|amount| string_field(amount, "currency"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_documented_trade_states() {
        assert_eq!(map_trade_state("SUCCESS"), PaymentStatus::Succeeded);
        assert_eq!(map_trade_state("REFUND"), PaymentStatus::Refunded);
        assert_eq!(map_trade_state("NOTPAY"), PaymentStatus::RequiresAction);
        assert_eq!(map_trade_state("CLOSED"), PaymentStatus::Canceled);
        assert_eq!(map_trade_state("REVOKED"), PaymentStatus::Canceled);
        assert_eq!(map_trade_state("USERPAYING"), PaymentStatus::Processing);
        assert_eq!(map_trade_state("PAYERROR"), PaymentStatus::Failed);
        assert_eq!(map_trade_state("SOMETHING_NEW"), PaymentStatus::Unknown);
        assert_eq!(map_trade_state(""), PaymentStatus::Unknown);
    }

    #[test]
    fn maps_documented_refund_states() {
        assert_eq!(map_refund_status("SUCCESS"), RefundStatus::Succeeded);
        assert_eq!(map_refund_status("PROCESSING"), RefundStatus::Pending);
        assert_eq!(map_refund_status("ABNORMAL"), RefundStatus::Failed);
        assert_eq!(map_refund_status("CLOSED"), RefundStatus::Canceled);
        assert_eq!(map_refund_status("SOMETHING_NEW"), RefundStatus::Unknown);
    }

    #[test]
    fn reads_amount_fields_from_the_nested_object() {
        let value = serde_json::json!({
            "amount": { "total": 100, "refund": 40, "currency": "CNY" }
        });
        assert_eq!(amount_field(&value, "total"), Some(100));
        assert_eq!(amount_field(&value, "refund"), Some(40));
        assert_eq!(amount_field(&value, "payer_refund"), None);
        assert_eq!(amount_currency(&value), Some("CNY"));
        assert_eq!(money_from_fen(100).minor, 100);
        assert_eq!(money_from_fen(100).currency, webx_pay::Currency::CNY);
    }
}
