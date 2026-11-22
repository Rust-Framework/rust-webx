//! Normalized payment and refund lifecycle states.

use serde::{Deserialize, Serialize};

/// Provider-independent payment state.
///
/// This is a *view*: the provider's own status string is preserved separately
/// in [`PaymentIntent::provider_status`](crate::PaymentIntent), and an unknown
/// provider state maps to [`PaymentStatus::Unknown`] rather than being guessed
/// into `Succeeded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentStatus {
    /// Created locally or at the provider, no client action yet.
    Created,
    /// The payer must complete an action (redirect, scan, app switch, 3DS).
    RequiresAction,
    /// Accepted and being processed by the provider.
    Processing,
    /// Authorized but not captured (manual capture providers only).
    Authorized,
    /// Funds captured / payment completed.
    Succeeded,
    /// Canceled/closed/voided before completion.
    Canceled,
    /// Failed explicitly (declined, expired, error).
    Failed,
    /// Fully refunded.
    Refunded,
    /// Partially refunded.
    PartiallyRefunded,
    /// Provider state not mapped by this adapter.
    Unknown,
}

impl PaymentStatus {
    /// Whether no further state change is expected in the normal flow.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Canceled | Self::Failed | Self::Refunded
        )
    }

    /// Whether the payment captured money.
    pub fn is_paid(&self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Refunded | Self::PartiallyRefunded
        )
    }

    /// Stable wire name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::RequiresAction => "requires_action",
            Self::Processing => "processing",
            Self::Authorized => "authorized",
            Self::Succeeded => "succeeded",
            Self::Canceled => "canceled",
            Self::Failed => "failed",
            Self::Refunded => "refunded",
            Self::PartiallyRefunded => "partially_refunded",
            Self::Unknown => "unknown",
        }
    }
}

/// Provider-independent refund state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefundStatus {
    /// Refund accepted, not settled.
    Pending,
    /// Refund completed.
    Succeeded,
    /// Refund rejected by the provider.
    Failed,
    /// Refund canceled.
    Canceled,
    /// Not mapped.
    Unknown,
}

impl RefundStatus {
    /// Whether the refund is final.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Canceled)
    }

    /// Stable wire name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::Unknown => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&PaymentStatus::RequiresAction).unwrap(),
            "\"requires_action\""
        );
        assert_eq!(
            serde_json::from_str::<PaymentStatus>("\"partially_refunded\"").unwrap(),
            PaymentStatus::PartiallyRefunded
        );
    }

    #[test]
    fn terminal_and_paid_views_are_consistent() {
        assert!(PaymentStatus::Succeeded.is_terminal());
        assert!(PaymentStatus::Succeeded.is_paid());
        assert!(PaymentStatus::PartiallyRefunded.is_paid());
        assert!(!PaymentStatus::Authorized.is_paid());
        assert!(!PaymentStatus::Authorized.is_terminal());
    }
}
