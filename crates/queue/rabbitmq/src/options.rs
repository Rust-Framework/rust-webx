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
    /// Fanout exchange broadcast events are published to / consumed from.
    ///
    /// Empty means "derive `"<exchange>.broadcast"`"; see
    /// `with_broadcast_exchange` fills it in.
    /// Every node binds its own exclusive auto-delete queue to it.
    pub broadcast_exchange: String,
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
            broadcast_exchange: String::new(),
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

    /// Fanout exchange for broadcast delivery (empty = derive
    /// `"<exchange>.broadcast"`).
    pub fn with_broadcast_exchange(mut self, exchange: impl Into<String>) -> Self {
        self.broadcast_exchange = exchange.into();
        self
    }

    /// The fanout exchange broadcast events are published to / consumed from.
    ///
    /// Returns [`broadcast_exchange`](RabbitMqOptions::broadcast_exchange) when
    /// it is set (non-blank); otherwise derives `"<exchange>.broadcast"` so
    /// enabling broadcast support never requires a second configuration key.
    pub(crate) fn effective_broadcast_exchange(&self) -> String {
        let configured = self.broadcast_exchange.trim();
        if configured.is_empty() {
            derived_broadcast_exchange(&self.exchange)
        } else {
            configured.to_string()
        }
    }
}

impl RabbitMqOptions {
    pub(crate) fn connect_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.connect_timeout_seconds.max(1))
    }
}

/// Default broadcast exchange for `exchange` (`"<exchange>.broadcast"`).
pub(crate) fn derived_broadcast_exchange(exchange: &str) -> String {
    format!("{exchange}.broadcast")
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
