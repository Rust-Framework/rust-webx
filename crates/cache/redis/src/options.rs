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
///
/// `add_redis` validates these options at host startup (before the first
/// request is accepted) and fails the start with a typed error instead of
/// silently running a misconfigured cache/consumer; the checks are local, they
/// never connect to Redis.
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

    /// Validate local configuration (no network I/O).
    ///
    /// The URL is parsed but not dialed; a typo therefore fails host startup
    /// with an actionable message instead of the first cache/health call.
    pub(crate) fn validate(&self) -> std::result::Result<(), String> {
        if self.url.trim().is_empty() {
            return Err("url must not be empty".to_string());
        }
        redis::Client::open(self.url.as_str())
            .map_err(|error| format!("url is not a valid Redis URL: {error}"))?;
        if let Some(events) = &self.events {
            events.validate()?;
        }
        Ok(())
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
    /// First stream id used when the consumer group is created.
    ///
    /// `"0"` (default) replays everything still retained, so an event published
    /// between process start and the consumer's first `XREADGROUP` is not
    /// silently skipped; handlers are at-least-once and must be idempotent.
    /// `"$"` keeps only messages published after the group is created (lower
    /// duplicate volume for brand-new groups over a stream that already has
    /// history). Ignored once the group exists.
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
            start_id: "0".to_string(),
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

    /// Validate local configuration (no network I/O).
    ///
    /// A malformed stream/group id or an out-of-range tuning value currently
    /// fails deep inside a background reconnect loop; catching it at host
    /// startup turns a silent, permanent retry into one actionable error.
    pub(crate) fn validate(&self) -> std::result::Result<(), String> {
        validate_key("events.stream", &self.stream)?;
        validate_key("events.group", &self.group)?;
        if !self.consumer.trim().is_empty() {
            validate_key("events.consumer", &self.consumer)?;
        }
        if !self.broadcast_channel.trim().is_empty() {
            validate_key("events.broadcast_channel", &self.broadcast_channel)?;
        }
        validate_start_id(&self.start_id)?;
        if self.max_len == 0 {
            return Err("events.max_len must be at least 1 event".to_string());
        }
        if self.read_count == 0 {
            return Err("events.read_count must be at least 1".to_string());
        }
        if self.block_ms < 100 {
            return Err("events.block_ms must be at least 100ms".to_string());
        }
        if self.claim_idle_ms == 0 {
            return Err("events.claim_idle_ms must be at least 1ms".to_string());
        }
        if self.claim_pending && self.max_deliveries == 0 {
            return Err(
                "events.max_deliveries must be at least 1 when claim_pending is enabled"
                    .to_string(),
            );
        }
        if !self.dead_letter_stream.trim().is_empty() {
            validate_key("events.dead_letter_stream", &self.dead_letter_stream)?;
            if self.dead_letter_stream.trim() == self.stream.trim() {
                return Err(
                    "events.dead_letter_stream must differ from events.stream (replay would loop)"
                        .to_string(),
                );
            }
        }
        Ok(())
    }
}

/// Default broadcast channel for `stream` (`"<stream>:broadcast"`).
pub(crate) fn derived_broadcast_channel(stream: &str) -> String {
    format!("{stream}:broadcast")
}

/// Validate a Redis key/channel option: non-blank, bounded, printable.
fn validate_key(name: &str, value: &str) -> std::result::Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    if value.len() > 512 {
        return Err(format!("{name} must be at most 512 bytes"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{name} must not contain control characters"));
    }
    Ok(())
}

/// `start_id` must be `$`, `0`, or a Redis stream id (`<ms>` / `<ms>-<seq>`).
fn validate_start_id(value: &str) -> std::result::Result<(), String> {
    let value = value.trim();
    if value == "$" || value == "0" {
        return Ok(());
    }
    let (millis, sequence) = match value.split_once('-') {
        Some((millis, sequence)) => (millis, Some(sequence)),
        None => (value, None),
    };
    let all_digits = |text: &str| !text.is_empty() && text.chars().all(|ch| ch.is_ascii_digit());
    if all_digits(millis) && sequence.map(all_digits).unwrap_or(true) {
        return Ok(());
    }
    Err("events.start_id must be \"$\", \"0\", or a stream id such as \"0-0\"".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_start_id_replays_retained_events() {
        assert_eq!(RedisEventsOptions::default().start_id, "0");
        RedisEventsOptions::default()
            .validate()
            .expect("defaults are valid");
    }

    #[test]
    fn invalid_event_options_are_rejected_with_actionable_messages() {
        let blank_stream = RedisEventsOptions {
            stream: "  ".to_string(),
            ..RedisEventsOptions::default()
        };
        assert!(blank_stream.validate().unwrap_err().contains("stream"));

        let bad_start = RedisEventsOptions {
            start_id: "latest".to_string(),
            ..RedisEventsOptions::default()
        };
        assert!(bad_start.validate().unwrap_err().contains("start_id"));

        let zero_deliveries = RedisEventsOptions {
            max_deliveries: 0,
            ..RedisEventsOptions::default()
        };
        assert!(zero_deliveries
            .validate()
            .unwrap_err()
            .contains("max_deliveries"));

        let looping_dlq = RedisEventsOptions {
            dead_letter_stream: "webx:events".to_string(),
            ..RedisEventsOptions::default()
        };
        assert!(looping_dlq
            .validate()
            .unwrap_err()
            .contains("dead_letter_stream"));
    }

    #[test]
    fn valid_start_ids_are_accepted() {
        for start_id in ["$", "0", "0-0", "1700000000000", "42-7"] {
            let options = RedisEventsOptions {
                start_id: start_id.to_string(),
                ..RedisEventsOptions::default()
            };
            options
                .validate()
                .unwrap_or_else(|error| panic!("start_id {start_id:?} rejected: {error}"));
        }
    }

    #[test]
    fn redis_options_validate_url_and_nested_events() {
        RedisOptions::new("redis://127.0.0.1:6379")
            .validate()
            .expect("valid URL");
        assert!(RedisOptions::new("not a url").validate().is_err());
        assert!(RedisOptions::new("")
            .with_events(RedisEventsOptions {
                stream: String::new(),
                ..RedisEventsOptions::default()
            })
            .validate()
            .is_err());
    }
}
