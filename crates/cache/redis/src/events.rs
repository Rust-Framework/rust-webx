use std::sync::Arc;
use std::time::{Duration, Instant};

use webx_core::error::{Error, Result};
use webx_core::hosted::{delay, BackgroundService, CancellationToken};
use webx_core::mediator::{EventDispatcher, EventMessage, IEventPublisher};

use crate::connection::{transport_error, RedisConnection};
use crate::options::RedisEventsOptions;

/// Redis Streams event publisher (`XADD`).
///
/// The stream is capped with `MAXLEN ~`, so a stalled consumer cannot grow Redis
/// memory without limit. Publishing is accepted by the server before it
/// returns; handler execution happens in [`RedisEventConsumer`].
pub struct RedisEventPublisher {
    connection: Arc<RedisConnection>,
    options: RedisEventsOptions,
}

impl RedisEventPublisher {
    /// Create a publisher over a shared connection.
    pub fn new(connection: Arc<RedisConnection>, options: RedisEventsOptions) -> Self {
        Self {
            connection,
            options,
        }
    }
}

#[async_trait::async_trait]
impl IEventPublisher for RedisEventPublisher {
    async fn publish(&self, message: EventMessage) -> Result<()> {
        let mut connection = self
            .connection
            .manager()
            .await
            .map_err(|error| Error::Status(503, format!("redis unavailable: {error}")))?;
        redis::cmd("XADD")
            .arg(&self.options.stream)
            .arg("MAXLEN")
            .arg("~")
            .arg(self.options.max_len.max(1))
            .arg("*")
            .arg("event")
            .arg(&message.event_name)
            .arg("payload")
            .arg(&message.payload)
            .query_async::<String>(&mut connection)
            .await
            .map(|_| ())
            .map_err(|error| transport_error("redis XADD failed", error))
    }
}

/// Redis Streams consumer group worker.
///
/// Reads `XREADGROUP` batches, dispatches each event through
/// [`EventDispatcher`], and `XACK`s only after the handlers returned. Failed
/// entries stay pending and are reclaimed by `XAUTOCLAIM` after
/// `claim_idle_ms`; delivery is at-least-once, so handlers must be idempotent.
pub struct RedisEventConsumer {
    connection: Arc<RedisConnection>,
    options: RedisEventsOptions,
    dispatcher: Arc<EventDispatcher>,
    consumer: String,
}

impl RedisEventConsumer {
    /// Create a consumer worker over a shared connection and dispatcher.
    pub fn new(
        connection: Arc<RedisConnection>,
        options: RedisEventsOptions,
        dispatcher: Arc<EventDispatcher>,
    ) -> Self {
        let consumer = options.consumer_name();
        Self {
            connection,
            options,
            dispatcher,
            consumer,
        }
    }

    /// Consumer name used in `XREADGROUP`/`XAUTOCLAIM`.
    pub fn consumer_name(&self) -> &str {
        &self.consumer
    }

    /// Replay up to `count` entries from `dead_letter_stream` back to `stream`.
    ///
    /// The entry is re-added with the original `event`/`payload`; the DLQ copy
    /// is deleted only after the `XADD` succeeds. A crash between the two steps
    /// can produce a duplicate, so this is at-least-once like normal delivery.
    /// Malformed DLQ entries are logged and left in place; they cannot be
    /// replayed safely.
    pub async fn replay_dead_letter(&self, count: usize) -> Result<usize> {
        let Some(dead_letter) = self.dead_letter_stream() else {
            return Err(Error::Status(
                503,
                "redis dead-letter replay requires a distinct dead_letter_stream".to_string(),
            ));
        };
        let mut connection = self
            .connection
            .manager()
            .await
            .map_err(|error| Error::Status(503, format!("redis unavailable: {error}")))?;
        let value: redis::Value = redis::cmd("XRANGE")
            .arg(dead_letter)
            .arg("-")
            .arg("+")
            .arg("COUNT")
            .arg(count.max(1))
            .query_async(&mut connection)
            .await
            .map_err(|error| transport_error("redis dead-letter XRANGE failed", error))?;
        let redis::Value::Array(entries) = value else {
            return Ok(0);
        };
        let parsed = parse_entries(&entries);
        for entry in parsed.malformed {
            tracing::warn!(
                entry = %entry,
                stream = dead_letter,
                "skipping malformed dead-letter entry during replay"
            );
        }

        let mut replayed = 0;
        for (id, message) in parsed.messages {
            redis::cmd("XADD")
                .arg(&self.options.stream)
                .arg("MAXLEN")
                .arg("~")
                .arg(self.options.max_len.max(1))
                .arg("*")
                .arg("event")
                .arg(&message.event_name)
                .arg("payload")
                .arg(&message.payload)
                .query_async::<String>(&mut connection)
                .await
                .map_err(|error| transport_error("redis dead-letter XADD failed", error))?;
            redis::cmd("XDEL")
                .arg(dead_letter)
                .arg(&id)
                .query_async::<i64>(&mut connection)
                .await
                .map_err(|error| transport_error("redis dead-letter XDEL failed", error))?;
            replayed += 1;
        }
        Ok(replayed)
    }

    async fn run_once(&self, ct: &CancellationToken) -> Result<()> {
        let mut connection = self
            .connection
            .manager()
            .await
            .map_err(|error| Error::Status(503, format!("redis unavailable: {error}")))?;
        ensure_group(&mut connection, &self.options).await?;

        let claim_interval = Duration::from_millis(self.options.claim_idle_ms.max(1_000));
        // Claim on startup so pending entries from a previous instance are
        // recovered immediately instead of after one full claim interval.
        let mut last_claim = if self.options.claim_pending {
            Instant::now()
                .checked_sub(claim_interval)
                .unwrap_or_else(Instant::now)
        } else {
            Instant::now()
        };

        loop {
            // Claim **between** reads. `XREADGROUP` is not cancel-safe: a
            // select! timer that drops an in-flight read after Redis moved the
            // entry to the PEL loses the reply (the entry then waits for a
            // reclaim). Keeping claims sequential makes delivery deterministic.
            if self.options.claim_pending && last_claim.elapsed() >= claim_interval {
                self.claim_all(&mut connection, ct).await?;
                last_claim = Instant::now();
            }

            let batch = tokio::select! {
                _ = ct.cancelled() => return Ok(()),
                batch = read_group(&mut connection, &self.options, &self.consumer) => batch?,
            };
            if let Some(batch) = batch {
                ack_malformed(&mut connection, &self.options, &batch.malformed).await?;
                self.deliver(&mut connection, batch.messages).await?;
            }
        }
    }

    /// Drain the pending list in cursor order (bounded by `read_count` per
    /// batch). Called between reads, never concurrently with one.
    async fn claim_all(
        &self,
        connection: &mut redis::aio::ConnectionManager,
        ct: &CancellationToken,
    ) -> Result<()> {
        let mut cursor = "0-0".to_string();
        loop {
            let (batch, deleted, next_cursor) =
                claim_pending(connection, &self.options, &self.consumer, &cursor).await?;
            // Entries trimmed from the stream but still in the PEL are removed
            // explicitly, otherwise the PEL grows without bound under
            // `MAXLEN ~`.
            for id in deleted {
                xack(connection, &self.options, &id).await?;
            }
            ack_malformed(connection, &self.options, &batch.malformed).await?;
            self.deliver(connection, batch.messages).await?;
            if ct.is_cancelled() || next_cursor == "0-0" || next_cursor == cursor {
                break;
            }
            cursor = next_cursor;
        }
        Ok(())
    }

    async fn deliver(
        &self,
        connection: &mut redis::aio::ConnectionManager,
        entries: Vec<(String, EventMessage)>,
    ) -> Result<()> {
        for (id, message) in entries {
            match self.dispatcher.dispatch(&message).await {
                Ok(()) => xack(connection, &self.options, &id).await?,
                Err(error) => {
                    // Do not ack immediately: the entry stays pending and is
                    // reclaimed by XAUTOCLAIM after `claim_idle_ms`. Once it
                    // exceeds `max_deliveries`, move it to the dead-letter
                    // stream (or drop it) so a poison event cannot cycle
                    // forever.
                    let attempts = pending_delivery_count(connection, &self.options, &id).await?;
                    let max_deliveries = self.options.max_deliveries.max(1) as u64;
                    if attempts >= max_deliveries {
                        if let Some(dead_letter) = self.dead_letter_stream() {
                            xadd_dead_letter(
                                connection,
                                dead_letter,
                                &self.options,
                                &message,
                                attempts,
                                &id,
                                &error,
                            )
                            .await?;
                            if let Some(metrics) = self.dispatcher.metrics() {
                                metrics.record_dead_lettered();
                            }
                        }
                        xack(connection, &self.options, &id).await?;
                        tracing::error!(
                            event = %message.event_name,
                            entry = %id,
                            attempts,
                            error = %error,
                            "redis event dropped after max deliveries"
                        );
                    } else {
                        tracing::error!(
                            event = %message.event_name,
                            entry = %id,
                            attempts,
                            error = %error,
                            "redis event handler failed; entry left pending"
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn dead_letter_stream(&self) -> Option<&str> {
        let stream = self.options.dead_letter_stream.trim();
        if stream.is_empty() {
            return None;
        }
        if stream == self.options.stream {
            tracing::warn!(
                stream,
                "Redis dead_letter_stream equals the source stream; dropping instead of looping"
            );
            return None;
        }
        Some(stream)
    }
}

#[async_trait::async_trait]
impl BackgroundService for RedisEventConsumer {
    async fn execute(&self, ct: CancellationToken) -> Result<()> {
        let mut backoff = Duration::from_secs(1);
        loop {
            if ct.is_cancelled() {
                return Ok(());
            }
            // Cancellation must interrupt a slow connect/command, not just the
            // idle wait between attempts.
            let result = tokio::select! {
                _ = ct.cancelled() => return Ok(()),
                result = self.run_once(&ct) => result,
            };
            match result {
                Ok(()) if ct.is_cancelled() => return Ok(()),
                Ok(()) => backoff = Duration::from_secs(1),
                Err(error) => {
                    if let Some(metrics) = self.dispatcher.metrics() {
                        metrics.record_consumer_reconnect();
                    }
                    tracing::warn!(
                        stream = %self.options.stream,
                        error = %error,
                        "redis event consumer lost its connection; reconnecting"
                    );
                }
            }
            if !delay(&ct, backoff).await {
                return Ok(());
            }
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }
}

async fn ensure_group(
    connection: &mut redis::aio::ConnectionManager,
    options: &RedisEventsOptions,
) -> Result<()> {
    let result = redis::cmd("XGROUP")
        .arg("CREATE")
        .arg(&options.stream)
        .arg(&options.group)
        .arg(&options.start_id)
        .arg("MKSTREAM")
        .query_async::<String>(connection)
        .await;
    match result {
        Ok(_) => Ok(()),
        // The group already exists (another instance created it). This is the
        // Redis protocol's documented idempotency signal, not a business error.
        Err(error) if error.to_string().contains("BUSYGROUP") => Ok(()),
        Err(error) => Err(transport_error("redis XGROUP CREATE failed", error)),
    }
}

async fn read_group(
    connection: &mut redis::aio::ConnectionManager,
    options: &RedisEventsOptions,
    consumer: &str,
) -> Result<Option<ParsedEntries>> {
    let value: redis::Value = redis::cmd("XREADGROUP")
        .arg("GROUP")
        .arg(&options.group)
        .arg(consumer)
        .arg("COUNT")
        .arg(options.read_count.max(1))
        .arg("BLOCK")
        // A zero block would prevent the claim pass between reads from ever
        // running; clamp to a short, still-cheap poll.
        .arg(options.block_ms.max(100))
        .arg("STREAMS")
        .arg(&options.stream)
        .arg(">")
        .query_async(connection)
        .await
        .map_err(|error| transport_error("redis XREADGROUP failed", error))?;
    Ok(parse_read_reply(&value))
}

async fn claim_pending(
    connection: &mut redis::aio::ConnectionManager,
    options: &RedisEventsOptions,
    consumer: &str,
    cursor: &str,
) -> Result<(ParsedEntries, Vec<String>, String)> {
    let value: redis::Value = redis::cmd("XAUTOCLAIM")
        .arg(&options.stream)
        .arg(&options.group)
        .arg(consumer)
        .arg(options.claim_idle_ms)
        .arg(cursor)
        .arg("COUNT")
        .arg(options.read_count.max(1))
        .query_async(connection)
        .await
        .map_err(|error| transport_error("redis XAUTOCLAIM failed", error))?;
    Ok(parse_claim_reply(&value))
}

async fn ack_malformed(
    connection: &mut redis::aio::ConnectionManager,
    options: &RedisEventsOptions,
    ids: &[String],
) -> Result<()> {
    for id in ids {
        xack(connection, options, id).await?;
        tracing::warn!(
            entry = %id,
            "redis event entry is malformed; acked so it cannot poison the PEL"
        );
    }
    Ok(())
}

async fn xack(
    connection: &mut redis::aio::ConnectionManager,
    options: &RedisEventsOptions,
    id: &str,
) -> Result<()> {
    redis::cmd("XACK")
        .arg(&options.stream)
        .arg(&options.group)
        .arg(id)
        .query_async::<i64>(connection)
        .await
        .map(|_| ())
        .map_err(|error| transport_error("redis XACK failed", error))
}

async fn pending_delivery_count(
    connection: &mut redis::aio::ConnectionManager,
    options: &RedisEventsOptions,
    id: &str,
) -> Result<u64> {
    let value: redis::Value = redis::cmd("XPENDING")
        .arg(&options.stream)
        .arg(&options.group)
        .arg(id)
        .arg(id)
        .arg(1)
        .query_async(connection)
        .await
        .map_err(|error| transport_error("redis XPENDING failed", error))?;
    Ok(parse_pending_count(&value))
}

async fn xadd_dead_letter(
    connection: &mut redis::aio::ConnectionManager,
    dead_letter: &str,
    options: &RedisEventsOptions,
    message: &EventMessage,
    attempts: u64,
    source_id: &str,
    error: &Error,
) -> Result<()> {
    redis::cmd("XADD")
        .arg(dead_letter)
        .arg("MAXLEN")
        .arg("~")
        .arg(options.max_len.max(1))
        .arg("*")
        .arg("event")
        .arg(&message.event_name)
        .arg("payload")
        .arg(&message.payload)
        .arg("attempts")
        .arg(attempts)
        .arg("source_id")
        .arg(source_id)
        .arg("error")
        .arg(error.to_string())
        .query_async::<String>(connection)
        .await
        .map(|_| ())
        .map_err(|error| transport_error("redis dead-letter XADD failed", error))
}

#[derive(Default)]
struct ParsedEntries {
    messages: Vec<(String, EventMessage)>,
    /// Entries whose id was readable but whose event/payload fields were not;
    /// they are acked so a malformed entry cannot poison the PEL.
    malformed: Vec<String>,
}

impl ParsedEntries {
    fn merge(&mut self, other: ParsedEntries) {
        self.messages.extend(other.messages);
        self.malformed.extend(other.malformed);
    }
}

fn parse_read_reply(value: &redis::Value) -> Option<ParsedEntries> {
    let redis::Value::Array(streams) = value else {
        // `XREADGROUP BLOCK` returns Nil on timeout.
        return None;
    };
    let mut parsed = ParsedEntries::default();
    for stream in streams {
        let redis::Value::Array(parts) = stream else {
            continue;
        };
        let Some(redis::Value::Array(batch)) = parts.get(1) else {
            continue;
        };
        parsed.merge(parse_entries(batch));
    }
    Some(parsed)
}

fn parse_claim_reply(value: &redis::Value) -> (ParsedEntries, Vec<String>, String) {
    let redis::Value::Array(parts) = value else {
        return (ParsedEntries::default(), Vec::new(), "0-0".to_string());
    };
    let entries = match parts.get(1) {
        Some(redis::Value::Array(entries)) => parse_entries(entries),
        _ => ParsedEntries::default(),
    };
    let deleted_ids = match parts.get(2) {
        Some(redis::Value::Array(ids)) => ids
            .iter()
            .filter_map(value_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    let next_cursor = parts
        .first()
        .and_then(value_str)
        .unwrap_or("0-0")
        .to_string();
    (entries, deleted_ids, next_cursor)
}

fn parse_pending_count(value: &redis::Value) -> u64 {
    let redis::Value::Array(parts) = value else {
        return 0;
    };
    // With an explicit `start end count` range, Redis returns the entry list
    // directly: `[[id, consumer, idle_ms, delivery_count], ...]`. The summary
    // form (`XPENDING key group`) wraps the same list at index 3.
    let entries: &[redis::Value] = if matches!(parts.first(), Some(redis::Value::Array(_))) {
        parts
    } else {
        match parts.get(3) {
            Some(redis::Value::Array(entries)) => entries,
            _ => return 0,
        }
    };
    let Some(redis::Value::Array(first)) = entries.first() else {
        return 0;
    };
    // The delivery count is always the last element of the entry.
    first.last().and_then(value_u64).unwrap_or(0)
}

fn value_u64(value: &redis::Value) -> Option<u64> {
    match value {
        redis::Value::Int(number) => u64::try_from(*number).ok(),
        redis::Value::BulkString(bytes) => std::str::from_utf8(bytes).ok()?.parse().ok(),
        redis::Value::SimpleString(text) => text.parse().ok(),
        _ => None,
    }
}

fn parse_entries(entries: &[redis::Value]) -> ParsedEntries {
    let mut parsed = ParsedEntries::default();
    for entry in entries {
        let redis::Value::Array(parts) = entry else {
            continue;
        };
        let Some(id) = parts.first().and_then(value_str) else {
            continue;
        };
        let Some(redis::Value::Array(fields)) = parts.get(1) else {
            parsed.malformed.push(id.to_string());
            continue;
        };

        let mut event_name = None;
        let mut payload = None;
        let mut index = 0;
        while index + 1 < fields.len() {
            match value_str(&fields[index]) {
                Some("event") => event_name = value_str(&fields[index + 1]).map(str::to_string),
                Some("payload") => payload = value_bytes(&fields[index + 1]),
                _ => {}
            }
            index += 2;
        }

        match (event_name, payload) {
            (Some(event_name), Some(payload)) => {
                parsed
                    .messages
                    .push((id.to_string(), EventMessage::new(event_name, payload)));
            }
            _ => parsed.malformed.push(id.to_string()),
        }
    }
    parsed
}

fn value_str(value: &redis::Value) -> Option<&str> {
    match value {
        redis::Value::BulkString(bytes) => std::str::from_utf8(bytes).ok(),
        redis::Value::SimpleString(text) => Some(text.as_str()),
        _ => None,
    }
}

fn value_bytes(value: &redis::Value) -> Option<Vec<u8>> {
    match value {
        redis::Value::BulkString(bytes) => Some(bytes.clone()),
        redis::Value::SimpleString(text) => Some(text.as_bytes().to_vec()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use redis::Value;

    fn entry(id: &str, event: &str, payload: &[u8]) -> Value {
        Value::Array(vec![
            Value::BulkString(id.as_bytes().to_vec()),
            Value::Array(vec![
                Value::BulkString(b"event".to_vec()),
                Value::BulkString(event.as_bytes().to_vec()),
                Value::BulkString(b"payload".to_vec()),
                Value::BulkString(payload.to_vec()),
            ]),
        ])
    }

    #[test]
    fn xreadgroup_reply_is_parsed_into_event_messages() {
        let reply = Value::Array(vec![Value::Array(vec![
            Value::BulkString(b"webx:events".to_vec()),
            Value::Array(vec![entry("1-0", "user.created", b"{\"id\":1}")]),
        ])]);
        let messages = parse_read_reply(&reply).expect("batch").messages;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].1.event_name, "user.created");
        assert_eq!(messages[0].1.payload, b"{\"id\":1}");
    }

    #[test]
    fn xautoclaim_reply_reports_deleted_ids_and_next_cursor() {
        let reply = Value::Array(vec![
            Value::BulkString(b"5-0".to_vec()),
            Value::Array(vec![entry("2-0", "user.created", b"{}")]),
            Value::Array(vec![Value::BulkString(b"1-0".to_vec())]),
        ]);
        let (batch, deleted, next_cursor) = parse_claim_reply(&reply);
        assert_eq!(batch.messages.len(), 1);
        assert_eq!(deleted, vec!["1-0".to_string()]);
        assert_eq!(next_cursor, "5-0");
    }

    #[test]
    fn malformed_entries_are_reported_for_acking() {
        let malformed = Value::Array(vec![
            Value::BulkString(b"3-0".to_vec()),
            Value::Array(vec![
                Value::BulkString(b"event".to_vec()),
                Value::BulkString(b"user.created".to_vec()),
                // payload is missing
            ]),
        ]);
        let parsed = parse_entries(&[malformed]);
        assert!(parsed.messages.is_empty());
        assert_eq!(parsed.malformed, vec!["3-0".to_string()]);
    }

    #[test]
    fn xpending_reply_exposes_the_delivery_count() {
        let reply = Value::Array(vec![Value::Array(vec![
            Value::BulkString(b"1-0".to_vec()),
            Value::BulkString(b"consumer".to_vec()),
            Value::Int(10),
            Value::Int(4),
        ])]);
        assert_eq!(parse_pending_count(&reply), 4);

        // Summary form (`XPENDING key group`) keeps entries at index 3.
        let summary = Value::Array(vec![
            Value::Int(1),
            Value::BulkString(b"1-0".to_vec()),
            Value::BulkString(b"1-0".to_vec()),
            Value::Array(vec![Value::Array(vec![
                Value::BulkString(b"consumer".to_vec()),
                Value::BulkString(b"4".to_vec()),
            ])]),
        ]);
        assert_eq!(parse_pending_count(&summary), 4);
    }

    #[test]
    fn empty_or_malformed_replies_are_ignored() {
        assert!(parse_read_reply(&Value::Nil).is_none());
        let (batch, deleted, cursor) = parse_claim_reply(&Value::Nil);
        assert!(batch.messages.is_empty());
        assert!(batch.malformed.is_empty());
        assert!(deleted.is_empty());
        assert_eq!(cursor, "0-0");
        assert!(parse_entries(&[Value::Int(1)]).messages.is_empty());
    }
}
