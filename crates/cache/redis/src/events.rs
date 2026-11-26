use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::sync::OnceCell;
use webx_core::error::{Error, Result};
use webx_core::hosted::{delay, BackgroundService, CancellationToken};
use webx_core::mediator::{EventDispatcher, EventMessage, IEventPublisher};

use crate::connection::{transport_error, RedisConnection};
use crate::options::RedisEventsOptions;

/// Redis Streams event publisher (`XADD` / PUBLISH).
///
/// Competing events go to the capped stream, so a stalled consumer cannot grow
/// Redis memory without limit. Broadcast events go to a **Pub/Sub channel**
/// (`RedisEventsOptions::broadcast_channel` derives `"<stream>:broadcast"` otherwise) because Pub/Sub has
/// fan-out semantics: every subscribed node receives its own copy. Publishing is
/// accepted by the server before it returns; handler execution happens in
/// [`RedisEventConsumer`].
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

    /// Channel broadcast events are published to (tests).
    #[cfg(test)]
    pub(crate) fn broadcast_channel(&self) -> String {
        self.options.effective_broadcast_channel()
    }

    async fn publish_to_stream(&self, message: EventMessage) -> Result<()> {
        let mut connection = self
            .connection
            .manager()
            .await
            .map_err(|error| Error::status(503, format!("redis unavailable: {error}")))?;
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

    async fn publish_to_channel(&self, message: EventMessage) -> Result<()> {
        // Broadcast-class events use the one binary-body wire framing so the
        // Pub/Sub payload carries both the routing name and the JSON body.
        let frame = message.to_wire()?;
        let channel = self.options.effective_broadcast_channel();
        let mut connection = self
            .connection
            .manager()
            .await
            .map_err(|error| Error::status(503, format!("redis unavailable: {error}")))?;
        redis::cmd("PUBLISH")
            .arg(&channel)
            .arg(frame)
            // PUBLISH returns the number of subscribed nodes; zero is not an
            // error (broadcast is at-most-once).
            .query_async::<i64>(&mut connection)
            .await
            .map(|_| ())
            .map_err(|error| transport_error("redis PUBLISH failed", error))
    }
}

#[async_trait::async_trait]
impl IEventPublisher for RedisEventPublisher {
    async fn publish(&self, message: EventMessage) -> Result<()> {
        self.publish_to_stream(message).await
    }

    /// Publish to the Pub/Sub broadcast channel, which every node's consumer
    /// subscribes to. Delivery is at-most-once and not ordered across nodes.
    async fn broadcast(&self, message: EventMessage) -> Result<()> {
        self.publish_to_channel(message).await
    }
}

/// Redis event consumer: Streams consumer group plus Pub/Sub broadcast channel.
///
/// The worker holds **two independent Redis connections**:
///
/// * the multiplexed connection used for `XREADGROUP`/`XAUTOCLAIM`, which reads
///   competing events, dispatches each through [`EventDispatcher`], and `XACK`s
///   only after the handlers returned. Failed entries stay pending and are
///   reclaimed by `XAUTOCLAIM` after `claim_idle_ms`; delivery is
///   at-least-once, so handlers must be idempotent.
/// * a dedicated Pub/Sub connection subscribed to
///   the derived broadcast channel (`"<stream>:broadcast"`), which reads
///   broadcast-class events and dispatches them through the same
///   [`EventDispatcher`]. Pub/Sub is at-most-once: undecodable frames and
///   handler errors are logged and dropped.
///
/// The two loops reconnect independently, so a broken Pub/Sub subscription can
/// never stop competing delivery (and vice versa). Both stop with the
/// [`CancellationToken`].
///
/// Competing reads use a **dedicated blocking connection** owned by this
/// consumer (no response timeout): blocking `XREADGROUP` on the shared
/// multiplexed handle would stall cache/publish/health commands and trip their
/// command timeout. The connection is created on first use and reconnects with
/// the loop.
///
/// `add_redis` registers this worker in DI as a singleton in addition to
/// starting it, so operations code can resolve `Arc<RedisEventConsumer>` and call
/// [`replay_dead_letter`](Self::replay_dead_letter).
pub struct RedisEventConsumer {
    connection: Arc<RedisConnection>,
    options: RedisEventsOptions,
    dispatcher: Arc<EventDispatcher>,
    consumer: String,
    /// Dedicated no-timeout connection for blocking `XREADGROUP`/`XAUTOCLAIM`.
    reader: OnceCell<redis::aio::ConnectionManager>,
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
            reader: OnceCell::new(),
        }
    }

    /// Pub/Sub channel broadcast events are consumed from (tests).
    #[cfg(test)]
    pub(crate) fn broadcast_channel(&self) -> String {
        self.options.effective_broadcast_channel()
    }

    /// The dedicated blocking-read connection (created on first use).
    async fn reader(&self) -> Result<redis::aio::ConnectionManager> {
        self.reader
            .get_or_try_init(|| async { self.connection.blocking_reader().await })
            .await
            .cloned()
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
            return Err(Error::status(
                503,
                "redis dead-letter replay requires a distinct dead_letter_stream".to_string(),
            ));
        };
        let mut connection = self
            .connection
            .manager()
            .await
            .map_err(|error| Error::status(503, format!("redis unavailable: {error}")))?;
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
                entry = %String::from_utf8_lossy(&entry),
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
        let mut connection = self.reader().await?;
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

    /// Consume the broadcast Pub/Sub channel until cancellation.
    ///
    /// A dedicated connection is required because a subscribed Redis connection
    /// cannot serve ordinary commands. The connection is re-established with
    /// backoff whenever it fails or the stream ends; errors only produce a log
    /// line and a reconnect counter, never abort the consumer.
    async fn run_broadcast(&self, ct: CancellationToken) -> Result<()> {
        let mut backoff = Duration::from_secs(1);
        loop {
            if ct.is_cancelled() {
                return Ok(());
            }
            // Cancellation must interrupt a slow connect/subscribe, not just the
            // idle wait between attempts.
            let result = tokio::select! {
                _ = ct.cancelled() => return Ok(()),
                result = self.consume_broadcast(&ct) => result,
            };
            match result {
                Ok(()) if ct.is_cancelled() => return Ok(()),
                Ok(()) => backoff = Duration::from_secs(1),
                Err(error) => {
                    if let Some(metrics) = self.dispatcher.metrics() {
                        metrics.record_consumer_reconnect();
                    }
                    tracing::warn!(
                        channel = %self.options.effective_broadcast_channel(),
                        error = %error,
                        "redis broadcast consumer lost its connection; reconnecting"
                    );
                }
            }
            if !delay(&ct, backoff).await {
                return Ok(());
            }
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }

    /// Subscribe to the broadcast channel and dispatch every frame.
    async fn consume_broadcast(&self, ct: &CancellationToken) -> Result<()> {
        let channel = self.options.effective_broadcast_channel();
        let mut pubsub = self.connection.pubsub().await?;
        pubsub
            .subscribe(&channel)
            .await
            .map_err(|error| transport_error("redis SUBSCRIBE failed", error))?;
        let mut messages = pubsub.into_on_message();
        loop {
            let message = tokio::select! {
                _ = ct.cancelled() => return Ok(()),
                message = messages.next() => message,
            };
            let Some(message) = message else {
                // The Pub/Sub task ended (connection closed); reconnect.
                return Err(Error::status(
                    503,
                    "redis pub/sub stream ended; reconnecting".to_string(),
                ));
            };
            match EventMessage::from_wire(message.get_payload_bytes()) {
                Ok(event) => {
                    if let Err(error) = self.dispatcher.dispatch(&event).await {
                        // Broadcast delivery is at-most-once: log and drop,
                        // never retry or requeue.
                        tracing::error!(
                            event = %event.event_name,
                            channel = %channel,
                            error = %error,
                            "redis broadcast event handler failed"
                        );
                    }
                }
                Err(error) => tracing::warn!(
                    channel = %channel,
                    error = %error,
                    "dropping malformed redis broadcast frame"
                ),
            }
        }
    }

    /// Consume the competing stream consumer group until cancellation.
    async fn run_streams(&self, ct: CancellationToken) -> Result<()> {
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

#[async_trait::async_trait]
impl BackgroundService for RedisEventConsumer {
    async fn execute(&self, ct: CancellationToken) -> Result<()> {
        // Always consume both delivery classes; `consumer_enabled` only decides
        // whether `add_redis` *registers* this worker (publisher-only services
        // set it to false). A manually started consumer behaves like the NATS /
        // RabbitMQ / Kafka adapters: both loops run until cancellation.
        let (streams, broadcast) =
            tokio::join!(self.run_streams(ct.clone()), self.run_broadcast(ct.clone()),);
        streams.and(broadcast)
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
    ids: &[Vec<u8>],
) -> Result<()> {
    for id in ids {
        redis::cmd("XACK")
            .arg(&options.stream)
            .arg(&options.group)
            .arg(id.as_slice())
            .query_async::<i64>(connection)
            .await
            .map_err(|error| transport_error("redis XACK (malformed entry) failed", error))?;
        tracing::warn!(
            entry = %String::from_utf8_lossy(id),
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
    parse_pending_count(&value).ok_or_else(|| {
        transport_error(
            "redis XPENDING reply could not be parsed",
            format!("entry {id} delivery count is unknown"),
        )
    })
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
    /// they are acked so a malformed entry cannot poison the PEL. The raw id
    /// bytes are kept: an id that is not valid UTF-8 must still be addressable
    /// by XACK.
    malformed: Vec<Vec<u8>>,
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

/// Delivery count from an `XPENDING` range reply.
///
/// `None` means the reply shape was not understood: callers must treat that as
/// a transport error. Returning `0` there would make a poison entry look like a
/// first delivery forever and never reach `max_deliveries`.
fn parse_pending_count(value: &redis::Value) -> Option<u64> {
    let redis::Value::Array(parts) = value else {
        return None;
    };
    // With an explicit `start end count` range, Redis returns the entry list
    // directly: `[[id, consumer, idle_ms, delivery_count], ...]`. The summary
    // form (`XPENDING key group`) wraps the same list at index 3.
    let entries: &[redis::Value] = if matches!(parts.first(), Some(redis::Value::Array(_))) {
        parts
    } else {
        match parts.get(3) {
            Some(redis::Value::Array(entries)) => entries,
            _ => return None,
        }
    };
    // An empty list is a valid "no pending entry" reply (the entry was already
    // acked by another consumer): report zero, not an error.
    let Some(redis::Value::Array(first)) = entries.first() else {
        return Some(0);
    };
    // The delivery count is always the last element of the entry.
    first.last().and_then(value_u64)
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
        // The raw id is needed for XACK even when it is not valid UTF-8.
        let Some(id) = parts.first().and_then(value_bytes) else {
            continue;
        };
        let id_text = String::from_utf8_lossy(&id).into_owned();
        let Some(redis::Value::Array(fields)) = parts.get(1) else {
            parsed.malformed.push(id);
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
                    .push((id_text, EventMessage::new(event_name, payload)));
            }
            _ => parsed.malformed.push(id),
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
        assert_eq!(parsed.malformed, vec![b"3-0".to_vec()]);
    }

    #[test]
    fn xpending_reply_exposes_the_delivery_count() {
        let reply = Value::Array(vec![Value::Array(vec![
            Value::BulkString(b"1-0".to_vec()),
            Value::BulkString(b"consumer".to_vec()),
            Value::Int(10),
            Value::Int(4),
        ])]);
        assert_eq!(parse_pending_count(&reply), Some(4));

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
        assert_eq!(parse_pending_count(&summary), Some(4));
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

    fn test_connection() -> Arc<RedisConnection> {
        Arc::new(RedisConnection::new(
            "redis://127.0.0.1:1",
            Duration::from_millis(200),
            Duration::from_millis(200),
        ))
    }

    fn test_dispatcher() -> Arc<EventDispatcher> {
        let provider = rust_dix::ServiceCollection::new()
            .build()
            .expect("test provider must build");
        Arc::new(EventDispatcher::new(provider))
    }

    #[test]
    fn broadcast_channel_defaults_to_a_derived_sibling() {
        let connection = test_connection();
        let options = RedisEventsOptions {
            stream: "app:events".to_string(),
            ..RedisEventsOptions::default()
        };
        assert_eq!(
            options.effective_broadcast_channel(),
            "app:events:broadcast"
        );

        let publisher = RedisEventPublisher::new(Arc::clone(&connection), options.clone());
        assert_eq!(publisher.broadcast_channel(), "app:events:broadcast");
        let consumer = RedisEventConsumer::new(Arc::clone(&connection), options, test_dispatcher());
        assert_eq!(consumer.broadcast_channel(), "app:events:broadcast");
    }

    #[test]
    fn an_explicit_broadcast_channel_wins_over_a_blank_one() {
        let options = RedisEventsOptions::default().with_broadcast_channel("webx.realtime");
        assert_eq!(options.effective_broadcast_channel(), "webx.realtime");

        // A blank override still derives from the stream rather than publishing
        // to an empty channel name.
        let options = RedisEventsOptions::default().with_broadcast_channel("   ");
        assert_eq!(
            options.effective_broadcast_channel(),
            "webx:events:broadcast"
        );
    }

    #[test]
    fn broadcast_channel_survives_configuration_round_trips() {
        // A configuration section that predates the field keeps the derived
        // default instead of failing to deserialize or using an empty channel.
        let options: RedisEventsOptions =
            serde_json::from_str(r#"{"stream":"app:events"}"#).expect("options deserialize");
        assert!(options.broadcast_channel.is_empty());
        assert_eq!(
            options.effective_broadcast_channel(),
            "app:events:broadcast"
        );

        let options = RedisEventsOptions {
            stream: "app:events".to_string(),
            broadcast_channel: "app.fanout".to_string(),
            ..RedisEventsOptions::default()
        };
        let json = serde_json::to_string(&options).expect("options serialize");
        let decoded: RedisEventsOptions = serde_json::from_str(&json).expect("options round trip");
        assert_eq!(decoded.effective_broadcast_channel(), "app.fanout");
    }

    #[test]
    fn broadcast_frames_round_trip_through_the_wire_format() {
        let message = EventMessage::new(
            "webx.realtime.fanout",
            br#"{"room":"lobby","payload":"hello"}"#.to_vec(),
        );
        let wire = message.to_wire().expect("encode broadcast frame");
        let decoded = EventMessage::from_wire(&wire).expect("decode broadcast frame");
        assert_eq!(decoded, message);
    }

    #[tokio::test]
    async fn both_consumer_loops_stop_when_the_token_is_cancelled() {
        // No Redis is listening on port 1, so this exercises the reconnect path
        // of both loops: failures are logged/retried and cancellation must end
        // them promptly.
        let options = RedisEventsOptions {
            stream: "webx:test:cancel".to_string(),
            group: "webx-test-cancel".to_string(),
            block_ms: 100,
            claim_idle_ms: 1_000,
            ..RedisEventsOptions::default()
        };
        let consumer = RedisEventConsumer::new(test_connection(), options, test_dispatcher());
        let ct = CancellationToken::new();
        let worker_ct = ct.clone();
        let task = tokio::spawn(async move { consumer.execute(worker_ct).await });

        tokio::time::sleep(Duration::from_millis(200)).await;
        ct.cancel();
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the consumer must stop after cancellation")
            .expect("the consumer task must not panic");
        assert!(result.is_ok(), "cancellation is a clean stop: {result:?}");
    }
}
