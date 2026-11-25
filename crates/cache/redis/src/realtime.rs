//! Redis Streams-backed real-time event log ([`IRealtimeEventStore`]).
//!
//! Every subscription gets a sequence key and a capped Stream:
//!
//! ```text
//! {prefix}:{subscription}:seq   INCR allocates the monotonic event id
//! {prefix}:{subscription}:log   XADD "{event_id}-0" payload created_at
//! ```
//!
//! The Stream id is **explicit** (`"{event_id}-0"`): its millisecond part is the
//! next value of the sequence key, so the `u64` resume cursor is shared by every
//! instance instead of being process-local. `XTRIM MAXLEN ~` bounds the retained
//! history; [`IRealtimeEventStore::resume`] reports `truncated` when the oldest
//! retained event is newer than the client cursor plus one (events after the
//! cursor were already evicted by retention).
//!
//! Redis Streams cannot enforce a **time** retention bound atomically with the
//! append, so this adapter supports **count retention only**; use the
//! PostgreSQL-backed implementation of the same port when time-based retention
//! is required.
//!
//! ```no_run
//! use std::sync::Arc;
//! use webx_core::realtime::{IRealtimeEventStore, ResumeRequest};
//! use webx_redis::{RedisConnection, RedisRealtimeEventStore};
//!
//! # async fn demo() -> webx_core::error::Result<()> {
//! let connection = Arc::new(RedisConnection::new(
//!     "redis://127.0.0.1:6379",
//!     std::time::Duration::from_secs(3),
//!     std::time::Duration::from_secs(2),
//! ));
//! let store = RedisRealtimeEventStore::new(connection);
//! let event = store.append("room-1", "{\"text\":\"hello\"}".to_string()).await?;
//! let frame = store.resume(ResumeRequest::new("room-1", Some(event.event_id))).await?;
//! # let _ = frame;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use webx_core::error::{Error, Result};
use webx_core::realtime::{IRealtimeEventStore, RealtimeEvent, ResumeFrame, ResumeRequest};
use webx_core::time::{IClock, SystemClock};

use crate::connection::{transport_error, RedisConnection};

/// Default key prefix for the real-time event log.
pub(crate) const DEFAULT_REALTIME_PREFIX: &str = "webx:rt";

/// Default number of retained events per subscription.
pub(crate) const DEFAULT_REALTIME_MAX_EVENTS: usize = 1024;

/// Redis Streams options for [`RedisRealtimeEventStore`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RedisRealtimeEventStoreOptions {
    /// Key prefix: each subscription uses `{prefix}:{subscription}:seq` and
    /// `{prefix}:{subscription}:log`. Use a distinct prefix per application so
    /// several services can share one Redis.
    pub prefix: String,
    /// Retained events per subscription (`XTRIM MAXLEN ~`), bounding Redis
    /// memory. Values below one are clamped to one event.
    pub max_events: usize,
}

impl Default for RedisRealtimeEventStoreOptions {
    fn default() -> Self {
        Self {
            prefix: DEFAULT_REALTIME_PREFIX.to_string(),
            max_events: DEFAULT_REALTIME_MAX_EVENTS,
        }
    }
}

impl RedisRealtimeEventStoreOptions {
    /// Default options (`webx:rt` prefix, 1024 retained events).
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the key prefix.
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Override the retained event count.
    pub fn with_max_events(mut self, max_events: usize) -> Self {
        self.max_events = max_events;
        self
    }
}

/// Atomically allocate the next event id and append it to the Stream.
///
/// The commands must run in one script: Redis rejects an explicit Stream id that
/// is not greater than the current top id, and two nodes appending concurrently
/// would otherwise interleave their `INCR`/`XADD` pairs and one `XADD` would
/// fail. If the sequence key was lost/reset (eviction, restore) the append
/// fails once, repairs the sequence from the Stream top, and the next append
/// continues without a permanent gap.
const APPEND_SCRIPT: &str = r#"
local id = redis.call('INCR', KEYS[1])
local added = redis.pcall('XADD', KEYS[2], id .. '-0', 'payload', ARGV[1], 'created_at', ARGV[2])
if type(added) == 'table' and added.err then
  local top = redis.call('XREVRANGE', KEYS[2], '+', '-', 'COUNT', 1)
  if #top > 0 then
    local top_id = tonumber(string.match(top[1][1], '^(%d+)'))
    if top_id and top_id >= id then
      redis.call('SET', KEYS[1], top_id)
    end
  end
  return redis.error_reply(added.err)
end
redis.call('XTRIM', KEYS[2], 'MAXLEN', '~', ARGV[3])
return id
"#;

/// One atomic snapshot for [`IRealtimeEventStore::resume`].
///
/// Reading the oldest id, the newest id, the sequence fallback and the replay
/// range in separate round trips lets a concurrent `XTRIM` slip between them and
/// hide a retention gap. The script returns
/// `{first_retained_ms, last_retained_ms, seq_fallback, entries}` from one view.
const RESUME_SCRIPT: &str = r#"
local first = redis.call('XRANGE', KEYS[1], '-', '+', 'COUNT', 1)
local last = redis.call('XREVRANGE', KEYS[1], '+', '-', 'COUNT', 1)
local first_ms = false
local last_ms = false
if #first > 0 then first_ms = tonumber(string.match(first[1][1], '^(%d+)')) end
if #last > 0 then last_ms = tonumber(string.match(last[1][1], '^(%d+)')) end
local seq = redis.call('GET', KEYS[2])
local entries = redis.call('XRANGE', KEYS[1], '(' .. ARGV[1], '+')
return {first_ms or false, last_ms or false, seq or false, entries}
"#;

/// Redis Streams-backed [`IRealtimeEventStore`].
///
/// The store is safe to share across nodes: the event id comes from a Redis
/// `INCR`, so a resume cursor is meaningful on every instance. Timestamps for
/// [`RealtimeEvent::created_at_unix_secs`] come from the injected [`IClock`]
/// (deterministic tests inject `TestClock`); they are diagnostics only and do
/// not drive retention.
pub struct RedisRealtimeEventStore {
    connection: Arc<RedisConnection>,
    options: RedisRealtimeEventStoreOptions,
    clock: Arc<dyn IClock>,
}

impl RedisRealtimeEventStore {
    /// Create a store with default options and the system clock.
    pub fn new(connection: Arc<RedisConnection>) -> Self {
        Self::with_options(connection, RedisRealtimeEventStoreOptions::default())
    }

    /// Create a store with explicit options and the system clock.
    pub fn with_options(
        connection: Arc<RedisConnection>,
        options: RedisRealtimeEventStoreOptions,
    ) -> Self {
        Self::with_clock(connection, options, Arc::new(SystemClock))
    }

    /// Create a store with an injected clock (deterministic tests).
    pub(crate) fn with_clock(
        connection: Arc<RedisConnection>,
        options: RedisRealtimeEventStoreOptions,
        clock: Arc<dyn IClock>,
    ) -> Self {
        Self {
            connection,
            options,
            clock,
        }
    }

    /// Retention/prefix options (tests).
    #[cfg(test)]
    pub(crate) fn options(&self) -> &RedisRealtimeEventStoreOptions {
        &self.options
    }

    /// Stream key holding the retained events of `subscription`.
    pub(crate) fn log_key(&self, subscription: &str) -> String {
        subscription_key(&self.options.prefix, subscription, "log")
    }

    /// Key holding the next event id of `subscription`.
    pub(crate) fn seq_key(&self, subscription: &str) -> String {
        subscription_key(&self.options.prefix, subscription, "seq")
    }

    fn max_events(&self) -> usize {
        self.options.max_events.max(1)
    }

    async fn manager(&self) -> Result<redis::aio::ConnectionManager> {
        self.connection
            .manager()
            .await
            .map_err(|error| Error::status(503, format!("redis unavailable: {error}")))
    }
}

#[async_trait]
impl IRealtimeEventStore for RedisRealtimeEventStore {
    async fn append(&self, subscription: &str, payload: String) -> Result<RealtimeEvent> {
        validate_subscription(subscription)?;
        let created_at_unix_secs = self.clock.now_unix();
        let mut connection = self.manager().await?;
        let event_id: u64 = redis::cmd("EVAL")
            .arg(APPEND_SCRIPT)
            .arg(2)
            .arg(self.seq_key(subscription))
            .arg(self.log_key(subscription))
            .arg(&payload)
            .arg(created_at_unix_secs)
            .arg(self.max_events())
            .query_async(&mut connection)
            .await
            .map_err(|error| transport_error("redis realtime append failed", error))?;
        Ok(RealtimeEvent {
            event_id,
            subscription: subscription.to_string(),
            payload,
            created_at_unix_secs,
        })
    }

    async fn resume(&self, request: ResumeRequest) -> Result<ResumeFrame> {
        validate_subscription(&request.subscription)?;
        let cursor = request.cursor.unwrap_or(0);
        let mut connection = self.manager().await?;

        // Oldest retained id, newest retained id, sequence fallback and the
        // replay range in one atomic view: separate round trips could observe a
        // concurrent XTRIM between them and hide a retention gap.
        let value: redis::Value = redis::cmd("EVAL")
            .arg(RESUME_SCRIPT)
            .arg(2)
            .arg(self.log_key(&request.subscription))
            .arg(self.seq_key(&request.subscription))
            .arg(cursor)
            .query_async(&mut connection)
            .await
            .map_err(|error| transport_error("redis realtime resume failed", error))?;
        let (first_retained, latest_retained, seq_fallback, events_reply) =
            parse_resume_reply(&value);
        let events = parse_events(&events_reply, &request.subscription);

        // `latest_retained` is the newest retained id; once retention emptied the
        // stream, the sequence key is the newest id ever assigned. A cursor ahead
        // of that came from another store epoch: report a gap and restart from the
        // latest known id instead of stalling forever.
        let last_assigned = latest_retained.or(seq_fallback).unwrap_or(0);
        let ahead = cursor > last_assigned;
        let evicted_through = evicted_through(first_retained, last_assigned);
        let truncated = ahead || is_truncated(cursor, first_retained, last_assigned);
        let next_cursor = if ahead {
            last_assigned
        } else {
            next_cursor(
                cursor,
                events.last().map(|event| event.event_id),
                evicted_through,
            )
        };
        Ok(ResumeFrame {
            subscription: request.subscription,
            events: if ahead { Vec::new() } else { events },
            next_cursor,
            truncated,
        })
    }

    async fn latest_cursor(&self, subscription: &str) -> u64 {
        if subscription.trim().is_empty() {
            tracing::warn!("latest_cursor called with an empty subscription");
            return 0;
        }
        let Ok(mut connection) = self.connection.manager().await else {
            tracing::warn!(subscription, "redis unavailable; latest_cursor returns 0");
            return 0;
        };
        let reply: std::result::Result<redis::Value, redis::RedisError> = redis::cmd("XREVRANGE")
            .arg(self.log_key(subscription))
            .arg("+")
            .arg("-")
            .arg("COUNT")
            .arg(1)
            .query_async(&mut connection)
            .await;
        match reply {
            Ok(value) => match parse_first_id(&value) {
                Some(id) => id,
                // Retention emptied the stream; the sequence key still knows the
                // newest id ever assigned.
                None => last_assigned_id(&mut connection, &self.seq_key(subscription)).await,
            },
            Err(error) => {
                tracing::warn!(subscription, error = %error, "redis XREVRANGE failed; latest_cursor returns 0");
                0
            }
        }
    }
}

/// `{prefix}:{subscription}:{suffix}` (or `{subscription}:{suffix}` without a
/// prefix).
fn subscription_key(prefix: &str, subscription: &str, suffix: &str) -> String {
    if prefix.is_empty() {
        format!("{subscription}:{suffix}")
    } else {
        format!("{prefix}:{subscription}:{suffix}")
    }
}

/// Fail closed on input that cannot address a log.
fn validate_subscription(subscription: &str) -> Result<()> {
    if subscription.trim().is_empty() {
        return Err(Error::validation(
            "a real-time event store subscription must be non-empty",
        ));
    }
    if subscription.len() > 512 {
        return Err(Error::validation(
            "a real-time event store subscription must be at most 512 bytes",
        ));
    }
    if subscription.chars().any(char::is_control) {
        return Err(Error::validation(
            "a real-time event store subscription must not contain control characters",
        ));
    }
    Ok(())
}

/// Truncation decision shared by `resume` and its unit tests.
///
/// `first_retained` is the oldest retained id. When it is newer than
/// `cursor + 1`, the events between them were evicted by retention. Without a
/// retained event, everything after `cursor` is gone only if the sequence key
/// advanced past it.
fn is_truncated(cursor: u64, first_retained: Option<u64>, last_assigned: u64) -> bool {
    match first_retained {
        Some(first) => first > cursor.saturating_add(1),
        None => last_assigned > cursor,
    }
}

/// Highest id known to be evicted by retention.
fn evicted_through(first_retained: Option<u64>, last_assigned: u64) -> u64 {
    match first_retained {
        Some(first) => first.saturating_sub(1),
        None => last_assigned,
    }
}

/// Cursor the client should present on the next reconnect.
///
/// Mirrors the in-memory store: the newest returned event id, or the request
/// cursor advanced past what retention evicted (so a caught-up client never
/// moves backwards).
fn next_cursor(cursor: u64, last_event: Option<u64>, evicted_through: u64) -> u64 {
    last_event.unwrap_or_else(|| cursor.max(evicted_through))
}

/// Decode the atomic [`RESUME_SCRIPT`] reply:
/// `{first_ms, last_ms, seq, entries}`.
fn parse_resume_reply(
    value: &redis::Value,
) -> (Option<u64>, Option<u64>, Option<u64>, redis::Value) {
    let redis::Value::Array(parts) = value else {
        return (None, None, None, redis::Value::Array(Vec::new()));
    };
    let number = |index: usize| -> Option<u64> {
        match parts.get(index) {
            Some(redis::Value::Int(value)) if *value >= 0 => Some(*value as u64),
            Some(redis::Value::BulkString(bytes)) => std::str::from_utf8(bytes)
                .ok()
                .and_then(|text| text.parse().ok()),
            _ => None,
        }
    };
    let entries = parts
        .get(3)
        .cloned()
        .unwrap_or(redis::Value::Array(Vec::new()));
    (number(0), number(1), number(2), entries)
}

/// Read the last id ever assigned from the sequence key (`0` when absent).
async fn last_assigned_id(connection: &mut redis::aio::ConnectionManager, seq_key: &str) -> u64 {
    let reply: std::result::Result<Option<u64>, redis::RedisError> =
        redis::cmd("GET").arg(seq_key).query_async(connection).await;
    match reply {
        Ok(value) => value.unwrap_or(0),
        Err(error) => {
            tracing::warn!(seq_key, error = %error, "redis GET of the realtime sequence key failed");
            0
        }
    }
}

/// Parse the millisecond part of a Redis Stream id (`"{millis}-{seq}"`).
fn parse_stream_id(id: &str) -> Option<u64> {
    id.split_once('-')?.0.parse().ok()
}

/// First entry id of an `XRANGE`/`XREVRANGE` reply.
fn parse_first_id(reply: &redis::Value) -> Option<u64> {
    let redis::Value::Array(entries) = reply else {
        return None;
    };
    let redis::Value::Array(parts) = entries.first()? else {
        return None;
    };
    parse_stream_id(parts.first().and_then(value_str)?)
}

/// Decode an `XRANGE` reply into retained events (malformed entries are
/// skipped and logged).
fn parse_events(reply: &redis::Value, subscription: &str) -> Vec<RealtimeEvent> {
    let redis::Value::Array(entries) = reply else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let parsed = parse_event(entry, subscription);
            if parsed.is_none() {
                tracing::warn!(
                    subscription,
                    "skipping malformed redis realtime stream entry"
                );
            }
            parsed
        })
        .collect()
}

fn parse_event(entry: &redis::Value, subscription: &str) -> Option<RealtimeEvent> {
    let redis::Value::Array(parts) = entry else {
        return None;
    };
    let event_id = parse_stream_id(parts.first().and_then(value_str)?)?;
    let redis::Value::Array(fields) = parts.get(1)? else {
        return None;
    };
    let mut payload = None;
    let mut created_at_unix_secs = None;
    let mut index = 0;
    while index + 1 < fields.len() {
        match value_str(&fields[index]) {
            Some("payload") => payload = value_str(&fields[index + 1]).map(str::to_string),
            Some("created_at") => {
                created_at_unix_secs =
                    value_str(&fields[index + 1]).and_then(|value| value.parse().ok())
            }
            _ => {}
        }
        index += 2;
    }
    Some(RealtimeEvent {
        event_id,
        subscription: subscription.to_string(),
        payload: payload?,
        created_at_unix_secs: created_at_unix_secs.unwrap_or(0),
    })
}

fn value_str(value: &redis::Value) -> Option<&str> {
    match value {
        redis::Value::BulkString(bytes) => std::str::from_utf8(bytes).ok(),
        redis::Value::SimpleString(text) => Some(text.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_connection() -> Arc<RedisConnection> {
        Arc::new(RedisConnection::new(
            "redis://127.0.0.1:1",
            std::time::Duration::from_millis(200),
            std::time::Duration::from_millis(200),
        ))
    }

    fn store() -> RedisRealtimeEventStore {
        RedisRealtimeEventStore::new(test_connection())
    }

    fn entry(id: &str, payload: &str, created_at: &str) -> redis::Value {
        redis::Value::Array(vec![
            redis::Value::BulkString(id.as_bytes().to_vec()),
            redis::Value::Array(vec![
                redis::Value::BulkString(b"payload".to_vec()),
                redis::Value::BulkString(payload.as_bytes().to_vec()),
                redis::Value::BulkString(b"created_at".to_vec()),
                redis::Value::BulkString(created_at.as_bytes().to_vec()),
            ]),
        ])
    }

    #[test]
    fn keys_are_namespaced_per_subscription() {
        let store = store();
        assert_eq!(store.options().prefix, "webx:rt");
        assert_eq!(store.log_key("room-1"), "webx:rt:room-1:log");
        assert_eq!(store.seq_key("room-1"), "webx:rt:room-1:seq");

        let store = RedisRealtimeEventStore::with_options(
            test_connection(),
            RedisRealtimeEventStoreOptions::default().with_prefix("app:rt"),
        );
        assert_eq!(store.log_key("room-1"), "app:rt:room-1:log");

        // An empty prefix keeps the keys valid (no leading colon).
        let store = RedisRealtimeEventStore::with_options(
            test_connection(),
            RedisRealtimeEventStoreOptions::default().with_prefix(""),
        );
        assert_eq!(store.log_key("room-1"), "room-1:log");
        assert_eq!(store.seq_key("room-1"), "room-1:seq");
    }

    #[test]
    fn options_have_bounded_defaults_and_config_defaults() {
        let options = RedisRealtimeEventStoreOptions::default();
        assert_eq!(options.prefix, DEFAULT_REALTIME_PREFIX);
        assert_eq!(options.max_events, DEFAULT_REALTIME_MAX_EVENTS);

        // A configuration section that predates the fields gets the defaults.
        let decoded: RedisRealtimeEventStoreOptions =
            serde_json::from_str("{}").expect("options deserialize");
        assert_eq!(decoded.prefix, DEFAULT_REALTIME_PREFIX);
        assert_eq!(decoded.max_events, DEFAULT_REALTIME_MAX_EVENTS);

        let decoded: RedisRealtimeEventStoreOptions =
            serde_json::from_str(r#"{"max_events":16}"#).expect("options deserialize");
        assert_eq!(decoded.max_events, 16);
    }

    #[test]
    fn stream_ids_expose_the_event_id_as_the_millisecond_part() {
        assert_eq!(parse_stream_id("42-0"), Some(42));
        assert_eq!(parse_stream_id("42-7"), Some(42));
        assert_eq!(parse_stream_id("not-an-id"), None);
        assert_eq!(parse_stream_id("-1-0"), None);
    }

    #[test]
    fn the_oldest_retained_id_is_read_from_the_first_entry() {
        let reply = redis::Value::Array(vec![entry("101-0", "a", "1700000000")]);
        assert_eq!(parse_first_id(&reply), Some(101));
        assert_eq!(parse_first_id(&redis::Value::Array(Vec::new())), None);
        assert_eq!(parse_first_id(&redis::Value::Nil), None);
    }

    #[test]
    fn xrange_replies_decode_into_retained_events() {
        let reply = redis::Value::Array(vec![
            entry("101-0", "one", "1700000001"),
            entry("102-0", "two", "1700000002"),
        ]);
        let events = parse_events(&reply, "room-1");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_id, 101);
        assert_eq!(events[0].payload, "one");
        assert_eq!(events[0].created_at_unix_secs, 1_700_000_001);
        assert_eq!(events[1].subscription, "room-1");

        // Malformed entries are skipped instead of failing the whole resume.
        let mixed = redis::Value::Array(vec![
            entry("103-0", "three", "1700000003"),
            redis::Value::Array(vec![
                redis::Value::BulkString(b"104-0".to_vec()),
                redis::Value::Array(vec![redis::Value::BulkString(b"payload".to_vec())]),
            ]),
            redis::Value::Int(1),
        ]);
        let events = parse_events(&mixed, "room-1");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_id, 103);
        assert!(parse_events(&redis::Value::Nil, "room-1").is_empty());
    }

    #[test]
    fn truncation_math_matches_the_memory_store() {
        // Cursor 0 + first retained 1: nothing was evicted.
        assert!(!is_truncated(0, Some(1), 0));
        // Cursor 0 + first retained 2: event 1 was evicted -> gap.
        assert!(is_truncated(0, Some(2), 0));
        // Cursor at the retained boundary is not a gap.
        assert!(!is_truncated(5, Some(6), 0));
        assert!(!is_truncated(5, Some(5), 0));
        // A retained id beyond cursor + 1 is a real gap.
        assert!(is_truncated(5, Some(7), 0));
        // No retained events: only a gap when the sequence advanced past the
        // cursor.
        assert!(!is_truncated(0, None, 0));
        assert!(is_truncated(0, None, 3));
        assert!(!is_truncated(3, None, 3));
        assert!(!is_truncated(4, None, 3));
    }

    #[test]
    fn next_cursor_moves_forward_and_never_back() {
        // Newest returned event wins.
        assert_eq!(next_cursor(5, Some(9), 4), 9);
        // Caught up: stay at the client cursor.
        assert_eq!(next_cursor(9, None, 4), 9);
        // Everything evicted: advance past the retained history.
        assert_eq!(next_cursor(2, None, 12), 12);
        assert_eq!(evicted_through(Some(7), 0), 6);
        assert_eq!(evicted_through(None, 7), 7);
        assert_eq!(evicted_through(Some(1), 0), 0);
    }

    #[tokio::test]
    async fn an_empty_subscription_is_a_typed_validation_error() {
        let store = store();
        let append_error = store
            .append("", "payload".to_string())
            .await
            .expect_err("an empty subscription cannot address a log");
        assert_eq!(append_error.status_code(), 400);

        let resume_error = store
            .resume(ResumeRequest::new("   ", None))
            .await
            .expect_err("an empty subscription cannot address a log");
        assert_eq!(resume_error.status_code(), 400);
        assert_eq!(store.latest_cursor("").await, 0);
    }
}
