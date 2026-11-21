use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Kafka event transport options.
///
/// This starter uses plaintext `host:port` bootstrap brokers; TLS/SASL and
/// consumer-group offset commits are outside its scope (see the crate docs).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct KafkaOptions {
    /// Bootstrap brokers, e.g. `["127.0.0.1:9092"]`.
    pub brokers: Vec<String>,
    /// Topic that carries every rust-webx event.
    pub topic: String,
    /// Partition used by this adapter. Each partition needs its own consumer.
    pub partition: i32,
    /// Start the background consumer (publisher-only services set `false`).
    pub consumer_enabled: bool,
    /// Where a fresh consumer starts.
    pub start_offset: KafkaStartOffset,
    /// Create the topic when it is missing (one partition by default).
    pub create_topic: bool,
    /// Partitions used when creating the topic.
    pub partitions: i32,
    /// Replication factor used when creating the topic.
    pub replication_factor: i16,
    /// Timeout for the first connection attempt, in seconds.
    pub connect_timeout_seconds: u64,
    /// Fetch long-poll wait, in milliseconds.
    pub max_wait_ms: i32,
}

impl Default for KafkaOptions {
    fn default() -> Self {
        Self {
            brokers: vec!["127.0.0.1:9092".to_string()],
            topic: "webx.events".to_string(),
            partition: 0,
            consumer_enabled: true,
            start_offset: KafkaStartOffset::Latest,
            create_topic: true,
            partitions: 1,
            replication_factor: 1,
            connect_timeout_seconds: 5,
            max_wait_ms: 500,
        }
    }
}

impl KafkaOptions {
    /// Options for one broker + topic.
    pub fn new(broker: impl Into<String>, topic: impl Into<String>) -> Self {
        Self {
            brokers: vec![broker.into()],
            topic: topic.into(),
            ..Self::default()
        }
    }

    /// Additional bootstrap brokers.
    pub fn with_brokers(mut self, brokers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.brokers = brokers.into_iter().map(Into::into).collect();
        self
    }

    /// Partition consumed/published by this adapter.
    pub fn with_partition(mut self, partition: i32) -> Self {
        self.partition = partition;
        self
    }

    /// Start a fresh consumer at the earliest or latest offset.
    pub fn with_start_offset(mut self, start_offset: KafkaStartOffset) -> Self {
        self.start_offset = start_offset;
        self
    }

    /// Disable the background consumer.
    pub fn publisher_only(mut self) -> Self {
        self.consumer_enabled = false;
        self
    }

    pub(crate) fn connect_timeout(&self) -> Duration {
        Duration::from_secs(self.connect_timeout_seconds.max(1))
    }
}

impl From<&str> for KafkaOptions {
    fn from(broker: &str) -> Self {
        Self::new(broker, "webx.events")
    }
}

impl From<String> for KafkaOptions {
    fn from(broker: String) -> Self {
        Self::new(broker, "webx.events")
    }
}

/// Where a fresh Kafka consumer starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KafkaStartOffset {
    /// Replay every retained record.
    Earliest,
    /// Only records produced after the consumer started.
    #[default]
    Latest,
}
