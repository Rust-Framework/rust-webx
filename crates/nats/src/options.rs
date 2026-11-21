use std::time::Duration;

use serde::{Deserialize, Serialize};

/// NATS event transport options.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct NatsOptions {
    /// NATS server URL(s), e.g. `nats://127.0.0.1:4222`.
    pub url: String,
    /// Subject every rust-webx event is published to.
    pub subject: String,
    /// Optional queue group: messages are load-balanced across instances in the
    /// same group instead of fanned out to every subscriber.
    pub queue_group: Option<String>,
    /// Start the background consumer (publisher-only services set `false`).
    pub consumer_enabled: bool,
    /// Connect timeout in seconds.
    pub connection_timeout_seconds: u64,
}

impl Default for NatsOptions {
    fn default() -> Self {
        Self {
            url: "nats://127.0.0.1:4222".to_string(),
            subject: "webx.events".to_string(),
            queue_group: None,
            consumer_enabled: true,
            connection_timeout_seconds: 5,
        }
    }
}

impl NatsOptions {
    /// Options for a single NATS server.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Self::default()
        }
    }

    /// Publish/subscribe subject.
    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    /// Load-balance deliveries across instances sharing `group`.
    pub fn with_queue_group(mut self, group: impl Into<String>) -> Self {
        self.queue_group = Some(group.into());
        self
    }

    /// Disable the background consumer.
    pub fn publisher_only(mut self) -> Self {
        self.consumer_enabled = false;
        self
    }

    pub(crate) fn connection_timeout(&self) -> Duration {
        Duration::from_secs(self.connection_timeout_seconds.max(1))
    }
}

impl From<&str> for NatsOptions {
    fn from(url: &str) -> Self {
        Self::new(url)
    }
}

impl From<String> for NatsOptions {
    fn from(url: String) -> Self {
        Self::new(url)
    }
}
