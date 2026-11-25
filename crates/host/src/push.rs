//! Capture push sender for tests and local development.
//!
//! [`InMemoryPushSender`] implements [`IPushSender`]
//! and keeps the most recent messages so tests can assert what a handler tried
//! to deliver. It validates the message shape exactly like a real adapter, never
//! touches the network, and bounds its buffer.
//!
//! Production registers a provider adapter with `HostBuilder::use_push`; the
//! host never installs a no-op sender by default, so a missing adapter fails at
//! DI resolution instead of silently dropping offline messages.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;

use webx_core::error::Result;
use webx_core::push::{IPushSender, PushMessage, PushOutcome};

/// Messages retained by the capture sender.
pub const DEFAULT_CAPTURE_PUSH_MESSAGES: usize = 100;

/// Captures push messages instead of delivering them.
pub struct InMemoryPushSender {
    max_messages: usize,
    outcome: PushOutcome,
    sent: Mutex<VecDeque<PushMessage>>,
}

impl Default for InMemoryPushSender {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryPushSender {
    /// A capture sender that keeps the last [`DEFAULT_CAPTURE_PUSH_MESSAGES`]
    /// messages and reports [`PushOutcome::Accepted`].
    pub fn new() -> Self {
        Self {
            max_messages: DEFAULT_CAPTURE_PUSH_MESSAGES,
            outcome: PushOutcome::Accepted,
            sent: Mutex::new(VecDeque::new()),
        }
    }

    /// Change how many messages are retained (oldest evicted first).
    pub fn with_max_messages(mut self, max_messages: usize) -> Self {
        self.max_messages = max_messages.max(1);
        self
    }

    /// Report `outcome` for every accepted message (for dead-token tests).
    pub fn with_outcome(mut self, outcome: PushOutcome) -> Self {
        self.outcome = outcome;
        self
    }

    /// Captured messages, oldest first.
    pub fn sent(&self) -> Vec<PushMessage> {
        self.lock().iter().cloned().collect()
    }

    /// Most recently captured message.
    pub fn last(&self) -> Option<PushMessage> {
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

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<PushMessage>> {
        self.sent.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl IPushSender for InMemoryPushSender {
    async fn send(&self, message: PushMessage) -> Result<PushOutcome> {
        message.validate()?;
        let mut sent = self.lock();
        sent.push_back(message);
        while sent.len() > self.max_messages {
            sent.pop_front();
        }
        Ok(self.outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webx_core::push::PushPlatform;

    #[tokio::test]
    async fn invalid_messages_are_rejected_before_capture() {
        let sender = InMemoryPushSender::new();
        let message = PushMessage::silent(PushPlatform::Fcm, "");
        let error = sender.send(message).await.expect_err("empty token");
        assert_eq!(error.code(), "validation_failed");
        assert!(sender.is_empty());
    }

    #[tokio::test]
    async fn messages_are_captured_in_order_and_bounded() {
        let sender = InMemoryPushSender::new().with_max_messages(2);
        for index in 0..3 {
            sender
                .send(PushMessage::silent(
                    PushPlatform::Apns,
                    format!("token-{index}"),
                ))
                .await
                .expect("send");
        }
        let sent = sender.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].token, "token-1");
        assert_eq!(sender.last().expect("last").token, "token-2");
    }
}
