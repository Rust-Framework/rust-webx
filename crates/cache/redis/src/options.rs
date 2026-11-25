use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Redis integration options.
///
/// The URL is a **single-node** endpoint (`redis://` / `rediss://`); Redis
/// Cluster and Sentinel need a dedicated adapter on the same ports.
///
/// The same connection is shared by the distributed cache and the event
/// transport. Bind it from configuration with `add_options::<RedisOptions>("Redis")`
/// or build it in code with [`RedisOptions::new`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RedisOptions {
    /// Redis URL (`redis://host:port/db` or `rediss://...`).
    pub url: String,
    /// Prefix applied to every cache key (the ASP.NET Core `InstanceName`).
    pub key_prefix: String,
    /// Timeout for the first connection attempt, in seconds.
    pub connect_timeout_seconds: u64,
    /// Per-command response timeout in seconds (a hung Redis must not pin a
    /// request or consumer forever).
    pub command_timeout_seconds: u64,
    /// Register the [`IDistributedCache`](webx_core::cache::IDistributedCache)
    /// implementation. Default: `true`.
    pub cache_enabled: bool,
    /// Default absolute cache TTL in seconds (`0` = entries never expire).
    pub default_ttl_seconds: u64,
    /// Sliding refresh window in seconds (`0` = disabled).
    pub sliding_ttl_seconds: u64,
    /// Event transport settings. `None` makes this a cache-only registration.
    pub events: Option<RedisEventsOptions>,
}

impl Default for RedisOptions {
    fn default() -> Self {
        Self {
            url: "redis://127.0.0.1:6379".to_string(),
            key_prefix: String::new(),
            connect_timeout_seconds: 5,
            command_timeout_seconds: 5,
            cache_enabled: true,
            default_ttl_seconds: 0,
            sliding_ttl_seconds: 0,
            events: Some(RedisEventsOptions::default()),
        }
    }
}

impl RedisOptions {
    /// Options for `redis://host:port/db`.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Self::default()
        }
    }

    /// Keep only the distributed cache (no event publisher/consumer).
    pub fn cache_only(mut self) -> Self {
        self.events = None;
        self
    }

    /// Disable the distributed cache; keep the event transport.
    pub fn without_cache(mut self) -> Self {
        self.cache_enabled = false;
        self
    }

    /// Prefix every cache key.
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.key_prefix = prefix.into();
        self
    }

    /// Default absolute cache TTL.
    pub fn with_default_ttl(mut self, ttl: Duration) -> Self {
        self.default_ttl_seconds = ttl.as_secs();
        self
    }

    /// Sliding refresh window applied on cache reads.
    pub fn with_sliding_ttl(mut self, ttl: Duration) -> Self {
        self.sliding_ttl_seconds = ttl.as_secs();
        self
    }

    /// Event transport settings.
    pub fn with_events(mut self, events: RedisEventsOptions) -> Self {
        self.events = Some(events);
        self
    }

    pub(crate) fn connect_timeout(&self) -> Duration {
        Duration::from_secs(self.connect_timeout_seconds.max(1))
    }

    pub(crate) fn command_timeout(&self) -> Duration {
        Duration::from_secs(self.command_timeout_seconds.max(1))
    }

    pub(crate) fn default_ttl(&self) -> Option<Duration> {
        (self.default_ttl_seconds > 0).then(|| Duration::from_secs(self.default_ttl_seconds))
    }

    pub(crate) fn sliding_ttl(&self) -> Option<Duration> {
        (self.sliding_ttl_seconds > 0).then(|| Duration::from_secs(self.sliding_ttl_seconds))
    }
}

impl From<&str> for RedisOptions {
    fn from(url: &str) -> Self {
        Self::new(url)
    }
}

impl From<String> for RedisOptions {
    fn from(url: String) -> Self {
        Self::new(url)
    }
}

/// Redis Streams event transport options.
///
/// `publish` writes to a capped stream and the consumer reads through a
/// consumer group, acknowledges only after [`EventDispatcher`](webx_core::mediator::EventDispatcher)
/// ran, and reclaims pending entries from crashed consumers.
///
/// `broadcast` (broadcast-class events such as `RealtimeFanoutEvent`) is
/// published to a **Pub/Sub channel** instead: every node's consumer subscribes
/// to it, so each node receives its own copy. Pub/Sub is at-most-once; a node
/// that is not subscribed at publish time misses the event.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RedisEventsOptions {
    /// Stream key (`XADD`/`XREADGROUP`).
    pub stream: String,
    /// Consumer group name.
    pub group: String,
    /// Consumer name. Empty = `webx-<pid>-<random>`; set it explicitly for dashboards.
    pub consumer: String,
    /// First stream id used when the consumer group is created (`$` = only new
    /// messages, `0` = replay everything retained). Ignored once the group
    /// exists.
    pub start_id: String,
    /// Pub/Sub channel broadcast-class events are published to / consumed from.
    ///
    /// Empty means "derive `"<stream>:broadcast"`"; `with_broadcast_channel`
    /// fills it in. Broadcast delivery never goes through the consumer group.
    pub broadcast_channel: String,
    /// Start the background consumer. A publisher-only service sets this to
    /// `false`.
    pub consumer_enabled: bool,
    /// Approximate stream cap (`XADD MAXLEN ~`), bounding memory.
    pub max_len: usize,
    /// Entries read per `XREADGROUP` round trip.
    pub read_count: usize,
    /// `XREADGROUP BLOCK` timeout in milliseconds.
    pub block_ms: u64,
    /// Age after which another consumer may claim a pending entry.
    pub claim_idle_ms: u64,
    /// Reclaim pending entries with `XAUTOCLAIM` (Redis 6.2+). Disable for
    /// older servers; new entries still flow through `XREADGROUP`, but pending
    /// retries, `max_deliveries` enforcement and PEL/deleted-id cleanup stop.
    pub claim_pending: bool,
    /// Drop an event after this many failed deliveries (prevents a poison
    /// message from cycling through the pending list forever).
    pub max_deliveries: u32,
    /// Optional stream that receives events dropped after `max_deliveries`
    /// (empty = drop with an error log).
    pub dead_letter_stream: String,
}

impl Default for RedisEventsOptions {
    fn default() -> Self {
        Self {
            stream: "webx:events".to_string(),
            group: "webx".to_string(),
            consumer: String::new(),
            start_id: "$".to_string(),
            broadcast_channel: String::new(),
            consumer_enabled: true,
            max_len: 100_000,
            read_count: 64,
            block_ms: 5_000,
            claim_idle_ms: 60_000,
            claim_pending: true,
            max_deliveries: 5,
            dead_letter_stream: String::new(),
        }
    }
}

impl RedisEventsOptions {
    /// Pub/Sub channel for broadcast delivery (blank = derive
    /// `"<stream>:broadcast"`).
    pub fn with_broadcast_channel(mut self, channel: impl Into<String>) -> Self {
        self.broadcast_channel = channel.into();
        self
    }

    /// The Pub/Sub channel broadcast events are published to / consumed from.
    ///
    /// Returns the configured channel when it is set (non-blank); otherwise
    /// derives `"<stream>:broadcast"` so enabling broadcast support never needs
    /// a second configuration key.
    pub(crate) fn effective_broadcast_channel(&self) -> String {
        let configured = self.broadcast_channel.trim();
        if configured.is_empty() {
            derived_broadcast_channel(&self.stream)
        } else {
            configured.to_string()
        }
    }

    /// Consumer name used by `XREADGROUP`/`XAUTOCLAIM`.
    pub(crate) fn consumer_name(&self) -> String {
        if !self.consumer.trim().is_empty() {
            return self.consumer.clone();
        }
        // Pods often share PID 1; add a random per-process suffix so each
        // replica is a distinct Redis consumer in the same group.
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        let token = RandomState::new().build_hasher().finish();
        format!("webx-{}-{token:016x}", std::process::id())
    }
}

/// Default broadcast channel for `stream` (`"<stream>:broadcast"`).
pub(crate) fn derived_broadcast_channel(stream: &str) -> String {
    format!("{stream}:broadcast")
}
