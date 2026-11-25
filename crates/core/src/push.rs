//! Mobile push notification port ([`IPushSender`]).
//!
//! Instant messaging is token-based: a message written while the recipient has
//! no live WebSocket must still reach the device through APNs/FCM/WebPush. The
//! framework owns the **port and its outcome model**, never a provider SDK:
//! adapters live in their own crates (`rust-webx-apns`,
//! `rust-webx-fcm`) and are registered with `HostBuilder::use_push`.
//!
//! # Errors vs provider verdicts
//!
//! A provider answers "accepted", "this device token is dead" or "try again
//! later". Those are **outcomes**, not transport failures: a dead token is
//! routine (the app unregisters it) and a `429` is expected under load. The
//! port therefore returns [`PushOutcome`] for provider verdicts and reserves
//! [`Error`] for transport/configuration failures (connection error, invalid
//! credentials, malformed request). Mapping a dead token to a `500` would make
//! every stale device look like a server incident.
//!
//! ```ignore
//! let push: Arc<dyn IPushSender> = provider.get::<dyn IPushSender>()?;
//! match push.send(PushMessage::alert(PushPlatform::Fcm, token, "Alice", "hi")).await? {
//!     PushOutcome::Accepted => {}
//!     PushOutcome::TokenInvalid => devices.delete(token).await?,
//!     PushOutcome::Retryable { retry_after } => schedule_retry(retry_after)?,
//! }
//! ```

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Platform the device token belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushPlatform {
    /// Apple Push Notification service (HTTP/2, token auth).
    Apns,
    /// Firebase Cloud Messaging (HTTP v1).
    Fcm,
    /// Web Push (VAPID) — the port accepts it; the reference adapters in this
    /// release cover APNs and FCM only.
    WebPush,
}

impl PushPlatform {
    /// Stable label for logs/metrics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apns => "apns",
            Self::Fcm => "fcm",
            Self::WebPush => "web_push",
        }
    }
}

/// Delivery priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushPriority {
    /// Background/quiet: the provider may batch or delay it.
    #[default]
    Normal,
    /// User-visible alert: may wake the device (APNs `10`, FCM `high`).
    High,
}

/// Visible notification block, when the push should render one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushNotification {
    /// Alert title.
    pub title: String,
    /// Alert body.
    pub body: String,
    /// Optional app badge value (APNs `badge`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<u32>,
    /// Optional sound name (APNs `sound`; FCM maps it to the platform default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound: Option<String>,
}

impl PushNotification {
    /// A title/body alert.
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            badge: None,
            sound: None,
        }
    }

    /// Attach a badge value.
    pub fn with_badge(mut self, badge: u32) -> Self {
        self.badge = Some(badge);
        self
    }

    /// Attach a sound name.
    pub fn with_sound(mut self, sound: impl Into<String>) -> Self {
        self.sound = Some(sound.into());
        self
    }

    fn validate(&self) -> Result<()> {
        if self.title.trim().is_empty() && self.body.trim().is_empty() {
            return Err(Error::validation(
                "a push notification needs a title or a body",
            ));
        }
        Ok(())
    }
}

/// One push message addressed to one device token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushMessage {
    /// Target platform (selects the provider adapter).
    pub platform: PushPlatform,
    /// Device token / endpoint (`APNs` hex token, FCM registration token, ...).
    pub token: String,
    /// Visible notification; `None` for a data-only (silent) push.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification: Option<PushNotification>,
    /// Application data payload (must be a JSON object for both APNs and FCM).
    #[serde(default)]
    pub data: serde_json::Value,
    /// Provider collapse key: a newer message replaces an undelivered one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapse_key: Option<String>,
    /// How long the provider may retain the message (`None` = provider default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<Duration>,
    /// Delivery priority.
    #[serde(default)]
    pub priority: PushPriority,
}

impl PushMessage {
    /// A data-only (silent) message.
    pub fn silent(platform: PushPlatform, token: impl Into<String>) -> Self {
        Self {
            platform,
            token: token.into(),
            notification: None,
            data: serde_json::Value::Object(serde_json::Map::new()),
            collapse_key: None,
            ttl: None,
            priority: PushPriority::Normal,
        }
    }

    /// A visible alert message.
    pub fn alert(
        platform: PushPlatform,
        token: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            notification: Some(PushNotification::new(title, body)),
            ..Self::silent(platform, token)
        }
    }

    /// Attach a JSON data payload.
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = data;
        self
    }

    /// Replace the visible notification block.
    pub fn with_notification(mut self, notification: PushNotification) -> Self {
        self.notification = Some(notification);
        self
    }

    /// Collapse undelivered messages under `key`.
    pub fn with_collapse_key(mut self, key: impl Into<String>) -> Self {
        self.collapse_key = Some(key.into());
        self
    }

    /// Provider retention bound.
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }

    /// Set the delivery priority.
    pub fn with_priority(mut self, priority: PushPriority) -> Self {
        self.priority = priority;
        self
    }

    /// Reject messages no provider could accept.
    ///
    /// Called by every adapter before it touches the network, so a shape error
    /// is the same typed `validation_failed` regardless of provider.
    pub fn validate(&self) -> Result<()> {
        if self.token.trim().is_empty() {
            return Err(Error::validation("a push message needs a device token"));
        }
        if let Some(ttl) = self.ttl {
            if ttl.is_zero() {
                return Err(Error::validation(
                    "a push TTL must be positive when set; omit it for the provider default",
                ));
            }
        }
        if !self.data.is_object() && !self.data.is_null() {
            return Err(Error::validation("push data must be a JSON object"));
        }
        if let Some(notification) = &self.notification {
            notification.validate()?;
        }
        Ok(())
    }
}

/// Provider verdict for one push attempt.
///
/// A transport/configuration failure is an [`Err`] instead; see the module docs
/// for why the distinction matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// The provider accepted the message for delivery.
    Accepted,
    /// The device token is no longer valid (APNs `BadDeviceToken`/`Unregistered`,
    /// FCM `UNREGISTERED`/`INVALID_ARGUMENT` for the token). The application
    /// should delete the device registration.
    TokenInvalid,
    /// The provider asked for a retry (rate limit, temporary unavailability).
    Retryable {
        /// `retry_after` from the provider, when it sent one.
        retry_after: Option<Duration>,
    },
}

impl PushOutcome {
    /// Whether the provider accepted the message.
    pub fn is_accepted(self) -> bool {
        matches!(self, Self::Accepted)
    }
}

/// Push delivery port.
///
/// Registered as a **singleton** with `HostBuilder::use_push`. No sender is
/// registered by default: a host that tries to push without an adapter fails at
/// DI resolution instead of silently dropping offline messages.
#[async_trait::async_trait]
pub trait IPushSender: Send + Sync {
    /// Deliver one message and map the provider verdict.
    ///
    /// `Err` means the message could not be delivered because of a transport or
    /// configuration failure (credentials rejected, endpoint unreachable,
    /// provider 5xx without a retry hint is reported as
    /// [`PushOutcome::Retryable`] when it is clearly retryable). Adapters must
    /// call [`PushMessage::validate`] first and must not log device tokens or
    /// credentials.
    async fn send(&self, message: PushMessage) -> Result<PushOutcome>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_without_a_token_is_a_validation_error() {
        let error = PushMessage::alert(PushPlatform::Apns, "", "t", "b")
            .validate()
            .expect_err("empty token must fail");
        assert_eq!(error.code(), "validation_failed");
    }

    #[test]
    fn a_notification_without_title_or_body_is_rejected() {
        let message = PushMessage::silent(PushPlatform::Fcm, "token")
            .with_notification(PushNotification::new(" ", ""));
        assert!(message.validate().is_err());
    }

    #[test]
    fn a_zero_ttl_is_rejected_but_absent_ttl_is_fine() {
        let message = PushMessage::silent(PushPlatform::Fcm, "token");
        assert!(message.validate().is_ok());
        assert!(message.with_ttl(Duration::ZERO).validate().is_err());
    }

    #[test]
    fn non_object_data_is_rejected() {
        let message =
            PushMessage::silent(PushPlatform::Fcm, "token").with_data(serde_json::json!("text"));
        assert!(message.validate().is_err());
    }

    #[test]
    fn the_outcome_reports_acceptance() {
        assert!(PushOutcome::Accepted.is_accepted());
        assert!(!PushOutcome::TokenInvalid.is_accepted());
        assert!(!PushOutcome::Retryable { retry_after: None }.is_accepted());
    }
}
