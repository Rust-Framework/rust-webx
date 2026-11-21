use serde::{Deserialize, Serialize};

/// RabbitMQ (AMQP) event transport options.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RabbitMqOptions {
    /// AMQP URL (`amqp://guest:guest@127.0.0.1:5672/%2f`).
    pub url: String,
    /// Timeout for the first connection attempt, in seconds.
    pub connect_timeout_seconds: u64,
    /// Durable topic exchange.
    pub exchange: String,
    /// Routing key used by publisher and consumer binding.
    pub routing_key: String,
    /// Durable queue. Services that need their own copy of every event use a
    /// distinct queue name; services sharing a queue load-balance deliveries.
    pub queue: String,
    /// Start the background consumer (publisher-only services set `false`).
    pub consumer_enabled: bool,
    /// Unacknowledged deliveries allowed per consumer.
    pub prefetch: u16,
    /// Quorum-queue delivery limit; a handler failure is requeued until this
    /// limit, then the broker dead-letters or drops the message.
    pub max_deliveries: u32,
    /// Optional dead-letter exchange for messages that exceed
    /// `max_deliveries` (empty = let RabbitMQ drop them after the limit).
    pub dead_letter_exchange: String,
}

impl Default for RabbitMqOptions {
    fn default() -> Self {
        Self {
            url: "amqp://guest:guest@127.0.0.1:5672/%2f".to_string(),
            connect_timeout_seconds: 5,
            exchange: "webx.events".to_string(),
            routing_key: "webx.events".to_string(),
            queue: "webx.events".to_string(),
            consumer_enabled: true,
            prefetch: 32,
            max_deliveries: 5,
            dead_letter_exchange: String::new(),
        }
    }
}

impl RabbitMqOptions {
    /// Options for an AMQP URL.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Self::default()
        }
    }

    /// Use a topic exchange / routing key / queue set.
    pub fn with_topology(
        mut self,
        exchange: impl Into<String>,
        routing_key: impl Into<String>,
        queue: impl Into<String>,
    ) -> Self {
        self.exchange = exchange.into();
        self.routing_key = routing_key.into();
        self.queue = queue.into();
        self
    }

    /// Disable the background consumer.
    pub fn publisher_only(mut self) -> Self {
        self.consumer_enabled = false;
        self
    }
}

impl RabbitMqOptions {
    pub(crate) fn connect_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.connect_timeout_seconds.max(1))
    }
}

impl From<&str> for RabbitMqOptions {
    fn from(url: &str) -> Self {
        Self::new(url)
    }
}

impl From<String> for RabbitMqOptions {
    fn from(url: String) -> Self {
        Self::new(url)
    }
}
