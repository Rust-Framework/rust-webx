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
///
/// # Structure-literal break (2.9.1)
///
/// `from`, `reply_to` and `headers` were added as public fields, so code that
/// builds this struct with a literal must list them after upgrading. Build
/// messages with [`EmailMessage::new`] plus the `with_*` builders instead: that
/// shape stays source-compatible when the framework adds optional fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailMessage {
    /// Primary recipient plus any extras (CC/BCC are folded into `to`).
    pub to: Vec<String>,
    /// Sender override; `None` lets the adapter use its configured default.
    pub from: Option<String>,
    /// Subject line.
    pub subject: String,
    /// Plain-text body (always present).
    pub text: String,
    /// Optional HTML alternative.
    pub html: Option<String>,
    /// Optional `Reply-To` override.
    pub reply_to: Option<String>,
    /// Additional transport headers, applied in order (for example
    /// `List-Unsubscribe`). Names and values are validated by
    /// [`EmailMessage::validate`].
    pub headers: Vec<(String, String)>,
}

impl EmailMessage {
    /// A plain-text email to one recipient.
    pub fn new(to: impl Into<String>, subject: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            to: vec![to.into()],
            from: None,
            subject: subject.into(),
            text: text.into(),
            html: None,
            reply_to: None,
            headers: Vec::new(),
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

    /// Set the sender for this message (for example a per-tenant or per-brand
    /// address).
    pub fn with_from(mut self, from: impl Into<String>) -> Self {
        self.from = Some(from.into());
        self
    }

    /// Set the `Reply-To` address.
    pub fn with_reply_to(mut self, reply_to: impl Into<String>) -> Self {
        self.reply_to = Some(reply_to.into());
        self
    }

    /// Append one transport header.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Validate the message shape (adapters call this before delivery).
    ///
    /// An address (`to`, `from`, `reply_to`) must be a non-empty,
    /// whitespace-free value containing `@`; the framework does not pretend to
    /// parse RFC 5322, but it rejects values that can only fail at the
    /// transport. A header name must be non-empty, whitespace/colon-free and
    /// free of control characters; a header value must be free of control
    /// characters — together this rejects CRLF/header injection.
    pub fn validate(&self) -> Result<()> {
        if self.to.is_empty() {
            return Err(crate::error::Error::Validation(
                "email message needs at least one recipient".to_string(),
            ));
        }
        for recipient in &self.to {
            validate_address(recipient, "recipient")?;
        }
        if let Some(from) = &self.from {
            validate_address(from, "sender")?;
        }
        if let Some(reply_to) = &self.reply_to {
            validate_address(reply_to, "reply-to")?;
        }
        for (name, value) in &self.headers {
            if name.trim().is_empty()
                || name.contains(char::is_whitespace)
                || name.contains(':')
                || name.chars().any(char::is_control)
            {
                return Err(crate::error::Error::Validation(
                    "invalid email header name".to_string(),
                ));
            }
            if value.chars().any(char::is_control) {
                return Err(crate::error::Error::Validation(format!(
                    "email header `{name}` value must not contain control characters"
                )));
            }
        }
        Ok(())
    }
}

/// Validate one email address field.
///
/// Shared by `to`, `from` and `reply_to`: trim, require an `@`, and reject any
/// whitespace (which covers CR/LF and tabs). The framework does not attempt an
/// RFC 5322 parse; this only rejects values that can never reach a transport.
fn validate_address(value: &str, role: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() || !value.contains('@') || value.contains(char::is_whitespace) {
        return Err(crate::error::Error::Validation(format!(
            "invalid email {role} `{value}`"
        )));
    }
    Ok(())
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

    #[test]
    fn email_message_carries_optional_sender_reply_to_and_headers() {
        let message = EmailMessage::new("alice@example.com", "hi", "body");
        assert_eq!(message.from, None);
        assert_eq!(message.reply_to, None);
        assert!(message.headers.is_empty());

        let message = message
            .with_from("billing@example.com")
            .with_reply_to("support@example.com")
            .with_header("List-Unsubscribe", "<https://example.com/unsubscribe>");
        assert_eq!(message.from.as_deref(), Some("billing@example.com"));
        assert_eq!(message.reply_to.as_deref(), Some("support@example.com"));
        assert_eq!(
            message.headers,
            vec![(
                "List-Unsubscribe".to_string(),
                "<https://example.com/unsubscribe>".to_string()
            )]
        );
        assert!(message.validate().is_ok());
    }

    #[test]
    fn email_message_rejects_unusable_sender_reply_to_and_headers() {
        let base = || EmailMessage::new("alice@example.com", "hi", "body");
        assert!(base().with_from("billing").validate().is_err());
        assert!(base().with_from("").validate().is_err());
        assert!(base().with_from("a b@example.com").validate().is_err());
        assert!(base().with_reply_to("support").validate().is_err());
        assert!(base()
            .with_reply_to("support@example.com\r\nBcc: x@example.com")
            .validate()
            .is_err());

        // Header injection: control characters in the name or the value can
        // never reach a transport, so validation rejects them.
        assert!(base().with_header("", "v").validate().is_err());
        assert!(base().with_header("bad name", "v").validate().is_err());
        assert!(base().with_header("X-Evil:", "v").validate().is_err());
        assert!(base().with_header("X-Evil\r\nBcc", "v").validate().is_err());
        assert!(base()
            .with_header("X-Test", "v\r\nBcc: x@example.com")
            .validate()
            .is_err());
        assert!(base().with_header("X-Test", "ok").validate().is_ok());
    }
}
