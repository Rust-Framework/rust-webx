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
    /// Subject broadcast events are published to / consumed from.
    ///
    /// Empty means "derive `"<subject>.broadcast"`"; see
    /// `with_broadcast_subject` fills it in.
    /// Broadcast delivery never uses the queue group.
    pub broadcast_subject: String,
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
            broadcast_subject: String::new(),
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

    /// Subject for broadcast delivery (empty = derive `"<subject>.broadcast"`).
    pub fn with_broadcast_subject(mut self, subject: impl Into<String>) -> Self {
        self.broadcast_subject = subject.into();
        self
    }

    /// The subject broadcast events are published to / consumed from.
    ///
    /// Returns [`broadcast_subject`](NatsOptions::broadcast_subject) when it is
    /// set (non-blank); otherwise derives `"<subject>.broadcast"` so enabling
    /// broadcast support never requires a second configuration key.
    pub(crate) fn effective_broadcast_subject(&self) -> String {
        let configured = self.broadcast_subject.trim();
        if configured.is_empty() {
            derived_broadcast_subject(&self.subject)
        } else {
            configured.to_string()
        }
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

/// Default broadcast subject for `subject` (`"<subject>.broadcast"`).
pub(crate) fn derived_broadcast_subject(subject: &str) -> String {
    format!("{subject}.broadcast")
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
