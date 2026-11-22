//! Capture senders for tests and local development.
//!
//! [`InMemoryEmailSender`] and [`InMemorySmsSender`] implement the
//! [`IEmailSender`]/[`ISmsSender`](webx_core::notifications::ISmsSender) ports
//! and keep the most recent messages so tests can assert what a handler tried to
//! deliver. They validate the message shape exactly like a real adapter, never
//! touch the network, and bound their buffer.
//!
//! Production deployments register a real adapter with
//! `HostBuilder::use_email_sender` / `use_sms_sender`; the host never installs a
//! no-op sender by default, so a missing adapter fails loudly at DI resolution
//! instead of silently dropping messages.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use webx_core::error::Result;
use webx_core::notifications::{EmailMessage, IEmailSender, ISmsSender, SmsMessage};

/// Messages retained by the capture senders.
pub const DEFAULT_CAPTURE_MESSAGES: usize = 100;

/// Captures emails instead of delivering them.
pub struct InMemoryEmailSender {
    max_messages: usize,
    sent: Mutex<VecDeque<EmailMessage>>,
}

impl Default for InMemoryEmailSender {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryEmailSender {
    /// A capture sender that keeps the last [`DEFAULT_CAPTURE_MESSAGES`] emails.
    pub fn new() -> Self {
        Self {
            max_messages: DEFAULT_CAPTURE_MESSAGES,
            sent: Mutex::new(VecDeque::new()),
        }
    }

    /// Change how many messages are retained (oldest evicted first).
    pub fn with_max_messages(mut self, max_messages: usize) -> Self {
        self.max_messages = max_messages.max(1);
        self
    }

    /// Captured emails, oldest first.
    pub fn sent(&self) -> Vec<EmailMessage> {
        self.lock().iter().cloned().collect()
    }

    /// Most recently captured email.
    pub fn last(&self) -> Option<EmailMessage> {
        self.lock().back().cloned()
    }

    /// Number of retained emails.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether nothing was sent yet.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Drop captured emails.
    pub fn clear(&self) {
        self.lock().clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<EmailMessage>> {
        self.sent.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl IEmailSender for InMemoryEmailSender {
    async fn send(&self, message: &EmailMessage) -> Result<()> {
        message.validate()?;
        let mut sent = self.lock();
        if sent.len() >= self.max_messages {
            sent.pop_front();
        }
        sent.push_back(message.clone());
        Ok(())
    }
}

/// Captures SMS messages instead of delivering them.
pub struct InMemorySmsSender {
    max_messages: usize,
    sent: Mutex<VecDeque<SmsMessage>>,
}

impl Default for InMemorySmsSender {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemorySmsSender {
    /// A capture sender that keeps the last [`DEFAULT_CAPTURE_MESSAGES`] messages.
    pub fn new() -> Self {
        Self {
            max_messages: DEFAULT_CAPTURE_MESSAGES,
            sent: Mutex::new(VecDeque::new()),
        }
    }

    /// Change how many messages are retained (oldest evicted first).
    pub fn with_max_messages(mut self, max_messages: usize) -> Self {
        self.max_messages = max_messages.max(1);
        self
    }

    /// Captured messages, oldest first.
    pub fn sent(&self) -> Vec<SmsMessage> {
        self.lock().iter().cloned().collect()
    }

    /// Most recently captured message.
    pub fn last(&self) -> Option<SmsMessage> {
        self.lock().back().cloned()
    }

    /// Number of retained messages.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether nothing was sent yet.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Drop captured messages.
    pub fn clear(&self) {
        self.lock().clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<SmsMessage>> {
        self.sent.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl ISmsSender for InMemorySmsSender {
    async fn send(&self, message: &SmsMessage) -> Result<()> {
        message.validate()?;
        let mut sent = self.lock();
        if sent.len() >= self.max_messages {
            sent.pop_front();
        }
        sent.push_back(message.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn email_sender_captures_and_validates_messages() {
        let sender = InMemoryEmailSender::new();
        sender
            .send(&EmailMessage::new(
                "alice@example.com",
                "Reset",
                "open the link",
            ))
            .await
            .expect("valid email");
        assert_eq!(sender.len(), 1);
        let captured = sender.last().expect("captured");
        assert_eq!(captured.subject, "Reset");
        assert_eq!(captured.to, vec!["alice@example.com".to_string()]);

        assert!(sender
            .send(&EmailMessage::new("not-an-address", "hi", "body"))
            .await
            .is_err());
        assert_eq!(sender.len(), 1, "invalid messages are not captured");

        sender.clear();
        assert!(sender.is_empty());
    }

    #[tokio::test]
    async fn email_sender_bounds_its_buffer() {
        let sender = InMemoryEmailSender::new().with_max_messages(2);
        for index in 0..3 {
            sender
                .send(&EmailMessage::new(
                    "alice@example.com",
                    format!("subject {index}"),
                    "body",
                ))
                .await
                .expect("send");
        }
        let sent = sender.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].subject, "subject 1");
        assert_eq!(sent[1].subject, "subject 2");
    }

    #[tokio::test]
    async fn sms_sender_captures_and_validates_messages() {
        let sender = InMemorySmsSender::new();
        sender
            .send(&SmsMessage::new("+15550100", "code 1234"))
            .await
            .expect("valid sms");
        assert_eq!(sender.last().expect("captured").text, "code 1234");

        assert!(sender
            .send(&SmsMessage::new("call me later", "code"))
            .await
            .is_err());
        assert_eq!(sender.len(), 1);
    }
}
