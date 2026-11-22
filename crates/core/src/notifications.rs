//! Outbound notification ports: email and SMS.
//!
//! The framework owns the **port and message shape**; delivery is an adapter
//! (SMTP, SES, Postmark, Twilio, ...) provided by the application or a starter.
//! `webx_host::notifications` ships a capture implementation for tests and
//! local development.
//!
//! # Deployment semantics
//!
//! * **Single process**: the port is stateless; any implementation replaces any
//!   other. A host never registers a default sender, so a message can never be
//!   silently swallowed by a framework no-op.
//! * **Multiple instances**: delivery is I/O outside the node. Durable,
//!   exactly-once delivery is an application concern (outbox + background
//!   service); a request handler should send inline only when the caller accepts
//!   at-most-once delivery.
//! * **Containers**: senders do not read configuration or open connections at
//!   construction — that belongs to the adapter. Readiness/liveness must not
//!   depend on a mail provider being reachable.
//! * **Performance**: `send` is async I/O; never call it while holding a lock or
//!   inside a database transaction. Implementations map transport failures to a
//!   retryable `503` so handlers can surface the error unchanged.
//!
//! ```ignore
//! let sender: Arc<dyn IEmailSender> = /* adapter or capture */;
//! sender
//!     .send(&EmailMessage::new("alice@example.com", "Reset", "…"))
//!     .await?;
//! ```

use async_trait::async_trait;

use crate::error::Result;

/// One outbound email.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailMessage {
    /// Primary recipient plus any extras (CC/BCC are folded into `to`).
    pub to: Vec<String>,
    /// Subject line.
    pub subject: String,
    /// Plain-text body (always present).
    pub text: String,
    /// Optional HTML alternative.
    pub html: Option<String>,
}

impl EmailMessage {
    /// A plain-text email to one recipient.
    pub fn new(to: impl Into<String>, subject: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            to: vec![to.into()],
            subject: subject.into(),
            text: text.into(),
            html: None,
        }
    }

    /// Add another recipient.
    pub fn with_recipient(mut self, to: impl Into<String>) -> Self {
        self.to.push(to.into());
        self
    }

    /// Attach an HTML alternative.
    pub fn with_html(mut self, html: impl Into<String>) -> Self {
        self.html = Some(html.into());
        self
    }

    /// Validate the message shape (adapters call this before delivery).
    ///
    /// A recipient must be a non-empty, whitespace-free value containing `@`;
    /// the framework does not pretend to parse RFC 5322, but it rejects values
    /// that can only fail at the transport.
    pub fn validate(&self) -> Result<()> {
        if self.to.is_empty() {
            return Err(crate::error::Error::Validation(
                "email message needs at least one recipient".to_string(),
            ));
        }
        for recipient in &self.to {
            let recipient = recipient.trim();
            if recipient.is_empty()
                || !recipient.contains('@')
                || recipient.contains(char::is_whitespace)
            {
                return Err(crate::error::Error::Validation(format!(
                    "invalid email recipient `{recipient}`"
                )));
            }
        }
        Ok(())
    }
}

/// One outbound SMS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmsMessage {
    /// Destination number in the adapter's expected format (E.164 recommended).
    pub to: String,
    /// Message body.
    pub text: String,
}

impl SmsMessage {
    /// A text message to one destination.
    pub fn new(to: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            to: to.into(),
            text: text.into(),
        }
    }

    /// Validate the message shape (adapters call this before delivery).
    pub fn validate(&self) -> Result<()> {
        let to = self.to.trim();
        if to.is_empty()
            || !to
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b' '))
        {
            return Err(crate::error::Error::Validation(format!(
                "invalid SMS destination `{to}`"
            )));
        }
        if self.text.trim().is_empty() {
            return Err(crate::error::Error::Validation(
                "SMS body must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

/// Email delivery port.
#[async_trait]
pub trait IEmailSender: Send + Sync {
    /// Deliver one email.
    ///
    /// Transport failures should come back as `Error::Status(503, …)` (retryable)
    /// with fixed client text; provider payloads stay in the operator log.
    async fn send(&self, message: &EmailMessage) -> Result<()>;
}

/// SMS delivery port.
#[async_trait]
pub trait ISmsSender: Send + Sync {
    /// Deliver one SMS.
    ///
    /// Same failure contract as [`IEmailSender::send`].
    async fn send(&self, message: &SmsMessage) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_message_validation_rejects_unusable_recipients() {
        assert!(EmailMessage::new("alice@example.com", "hi", "body")
            .validate()
            .is_ok());
        assert!(EmailMessage::new("alice", "hi", "body").validate().is_err());
        assert!(EmailMessage::new("", "hi", "body").validate().is_err());
        assert!(EmailMessage::new("a b@example.com", "hi", "body")
            .validate()
            .is_err());

        let mut message = EmailMessage::new("alice@example.com", "hi", "body");
        message.to.clear();
        assert!(message.validate().is_err());

        let with_html = EmailMessage::new("alice@example.com", "hi", "body")
            .with_html("<p>body</p>")
            .with_recipient("bob@example.com");
        assert_eq!(with_html.to.len(), 2);
        assert_eq!(with_html.html.as_deref(), Some("<p>body</p>"));
        assert!(with_html.validate().is_ok());
    }

    #[test]
    fn sms_message_validation_checks_destination_and_body() {
        assert!(SmsMessage::new("+15550100", "code 1234").validate().is_ok());
        assert!(SmsMessage::new("", "code").validate().is_err());
        assert!(SmsMessage::new("call me", "code").validate().is_err());
        assert!(SmsMessage::new("+15550100", "  ").validate().is_err());
    }
}
