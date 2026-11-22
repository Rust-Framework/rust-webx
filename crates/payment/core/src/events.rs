//! Normalized callback events and their event-bus envelope.

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use webx_core::mediator::EventMessage;

use crate::error::PaymentError;
use crate::ids::{AccountId, ProviderId};
use crate::money::Money;

/// Event-name prefix used when publishing to the framework event bus.
pub const PAYMENT_EVENT_PREFIX: &str = "payments.";

/// What happened, normalized across providers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PaymentEventType {
    /// Payment succeeded (funds captured).
    Succeeded,
    /// Authorization succeeded but capture is still pending.
    AuthorizationSucceeded,
    /// Payment failed/declined/expired.
    Failed,
    /// Payment canceled/closed.
    Canceled,
    /// Refund succeeded.
    RefundSucceeded,
    /// Refund failed.
    RefundFailed,
    /// Dispute/chargeback opened.
    DisputeOpened,
    /// Provider event type not mapped by this adapter.
    Unknown(String),
}

impl PaymentEventType {
    /// Wire name (`"succeeded"`, `"refund_succeeded"`, ...).
    pub fn as_str(&self) -> &str {
        match self {
            Self::Succeeded => "succeeded",
            Self::AuthorizationSucceeded => "authorization_succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::RefundSucceeded => "refund_succeeded",
            Self::RefundFailed => "refund_failed",
            Self::DisputeOpened => "dispute_opened",
            Self::Unknown(name) => name,
        }
    }

    /// Event-bus name (`"payments.succeeded"`, ...).
    pub fn event_name(&self) -> String {
        format!("{PAYMENT_EVENT_PREFIX}{}", self.as_str())
    }
}

impl fmt::Display for PaymentEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PaymentEventType {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value.trim() {
            "succeeded" => Self::Succeeded,
            "authorization_succeeded" => Self::AuthorizationSucceeded,
            "failed" => Self::Failed,
            "canceled" => Self::Canceled,
            "refund_succeeded" => Self::RefundSucceeded,
            "refund_failed" => Self::RefundFailed,
            "dispute_opened" => Self::DisputeOpened,
            other => Self::Unknown(other.to_string()),
        })
    }
}

/// A normalized callback event ready for business processing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaymentEvent {
    /// Provider that sent the callback.
    pub provider: ProviderId,
    /// Merchant account the callback belongs to, when known.
    pub account: Option<AccountId>,
    /// Provider event id, when the provider has one.
    pub provider_event_id: Option<String>,
    /// Raw provider status carried by the event, when the payload has one.
    pub provider_status: Option<String>,
    /// Stable de-duplication key (contract, see adapter docs).
    pub dedupe_key: String,
    /// Normalized event type.
    pub event_type: PaymentEventType,
    /// Merchant order id, when the callback carries it.
    pub order_id: Option<String>,
    /// Provider payment id, when the callback carries it.
    pub provider_payment_id: Option<String>,
    /// Amount, when the callback carries it.
    pub amount: Option<Money>,
    /// Event time (Unix seconds).
    pub occurred_at: i64,
    /// Whether the event came from live mode (when the provider says so).
    pub livemode: Option<bool>,
    /// Raw provider event, bounded by the webhook body limit; useful for audit.
    pub raw: serde_json::Value,
}

impl PaymentEvent {
    /// Build an event with the required fields.
    pub fn new(
        provider: ProviderId,
        event_type: PaymentEventType,
        dedupe_key: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            account: None,
            provider_event_id: None,
            provider_status: None,
            dedupe_key: dedupe_key.into(),
            event_type,
            order_id: None,
            provider_payment_id: None,
            amount: None,
            occurred_at: 0,
            livemode: None,
            raw: serde_json::Value::Null,
        }
    }

    /// Event-bus name for this event (`payments.succeeded`).
    pub fn event_name(&self) -> String {
        self.event_type.event_name()
    }

    /// Encode the event for [`webx_core::mediator::event::IEventPublisher`].
    pub fn to_event_message(&self) -> Result<EventMessage, PaymentError> {
        let payload = serde_json::to_vec(self).map_err(|error| {
            PaymentError::Serialization(format!("failed to encode payment event: {error}"))
        })?;
        Ok(EventMessage::new(self.event_name(), payload))
    }
}

impl Serialize for PaymentEventType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PaymentEventType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EventVisitor;
        impl Visitor<'_> for EventVisitor {
            type Value = PaymentEventType;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a payment event type")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                PaymentEventType::from_str(value).map_err(E::custom)
            }
        }
        deserializer.deserialize_str(EventVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_names_are_stable() {
        assert_eq!(
            PaymentEventType::RefundSucceeded.event_name(),
            "payments.refund_succeeded"
        );
        assert_eq!(
            PaymentEventType::Unknown("authorized".into()).event_name(),
            "payments.authorized"
        );
    }

    #[test]
    fn event_message_roundtrips() {
        let mut event = PaymentEvent::new(ProviderId::Stripe, PaymentEventType::Succeeded, "evt_1");
        event.order_id = Some("ORDER-1".into());
        let message = event.to_event_message().unwrap();
        assert_eq!(message.event_name, "payments.succeeded");
        let decoded: PaymentEvent = serde_json::from_slice(&message.payload).unwrap();
        assert_eq!(decoded.event_type, PaymentEventType::Succeeded);
    }
}
