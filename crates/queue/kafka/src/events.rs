use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use rskafka::chrono::Utc;
use rskafka::client::consumer::{StartOffset, StreamConsumerBuilder};
use rskafka::client::partition::{Compression, PartitionClient, UnknownTopicHandling};
use rskafka::client::Client;
use rskafka::record::Record;
use tokio::sync::Mutex;
use webx_core::error::{Error, Result};
use webx_core::hosted::{delay, BackgroundService, CancellationToken};
use webx_core::mediator::{EventDispatcher, EventMessage, IEventPublisher};

use crate::connection::KafkaConnection;
use crate::options::{KafkaOptions, KafkaStartOffset};

/// Kafka producer for one partition.
///
/// The adapter is a **single-partition follower**: the producer writes to
/// `options.partition`, and the consumer reads that partition from
/// `options.start_offset`. It does **not** commit consumer-group offsets; a
/// restart resumes from `start_offset` (so `Latest` skips downtime and
/// `Earliest` replays retention). Scale by running one instance per partition;
/// a group-aware adapter can replace this one without changing the
/// `IEventPublisher` / `EventDispatcher` contract.
///
/// Broadcast delivery is **not supported** and is not faked: Kafka has no
/// "every consumer gets a copy" primitive in this follower model, so
/// [`IEventPublisher::broadcast`] keeps its fail-closed default (an explicit
/// error) instead of silently degrading to single-node competing delivery.
pub struct KafkaEventPublisher {
    connection: Arc<KafkaConnection>,
    options: KafkaOptions,
    partition: Mutex<Option<Arc<PartitionClient>>>,
}

impl KafkaEventPublisher {
    /// Create a publisher over a shared client.
    pub fn new(connection: Arc<KafkaConnection>, options: KafkaOptions) -> Self {
        Self {
            connection,
            options,
            partition: Mutex::new(None),
        }
    }

    async fn partition(&self) -> Result<Arc<PartitionClient>> {
        let client = self.connection.client().await?;
        ensure_topic(&client, &self.options).await?;

        let mut slot = self.partition.lock().await;
        if let Some(partition) = slot.as_ref() {
            return Ok(Arc::clone(partition));
        }
        let partition = client
            .partition_client(
                &self.options.topic,
                self.options.partition,
                UnknownTopicHandling::Retry,
            )
            .await
            .map_err(|error| {
                Error::status(
                    503,
                    format!(
                        "Kafka partition {}/{} is unavailable: {error}",
                        self.options.topic, self.options.partition
                    ),
                )
            })?;
        let partition = Arc::new(partition);
        *slot = Some(Arc::clone(&partition));
        Ok(partition)
    }
}

#[async_trait::async_trait]
impl IEventPublisher for KafkaEventPublisher {
    async fn publish(&self, message: EventMessage) -> Result<()> {
        let frame = message.to_wire()?;
        let record = Record {
            key: None,
            value: Some(frame),
            headers: BTreeMap::new(),
            timestamp: rskafka::chrono::DateTime::<Utc>::from_timestamp_millis(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_millis() as i64)
                    .unwrap_or(0),
            )
            .unwrap_or_default(),
        };
        self.partition()
            .await?
            .produce(vec![record], Compression::NoCompression)
            .await
            .map(|_| ())
            .map_err(|error| Error::status(503, format!("Kafka produce failed: {error}")))
    }
}

/// Kafka partition consumer that dispatches records through [`EventDispatcher`].
///
/// Handler failures are logged and the consumer continues (the record is not
/// replayed by this adapter). Reconnects restart at `options.start_offset`, so
/// use `Earliest` when replaying is safe and `Latest` for live-only delivery.
pub struct KafkaEventConsumer {
    connection: Arc<KafkaConnection>,
    options: KafkaOptions,
    dispatcher: Arc<EventDispatcher>,
}

impl KafkaEventConsumer {
    /// Create a consumer worker over a shared client and dispatcher.
    pub fn new(
        connection: Arc<KafkaConnection>,
        options: KafkaOptions,
        dispatcher: Arc<EventDispatcher>,
    ) -> Self {
        Self {
            connection,
            options,
            dispatcher,
        }
    }

    async fn run_once(&self, ct: &CancellationToken) -> Result<()> {
        let client = self.connection.client().await?;
        ensure_topic(&client, &self.options).await?;
        let partition = Arc::new(
            client
                .partition_client(
                    &self.options.topic,
                    self.options.partition,
                    UnknownTopicHandling::Retry,
                )
                .await
                .map_err(|error| {
                    Error::status(
                        503,
                        format!(
                            "Kafka partition {}/{} is unavailable: {error}",
                            self.options.topic, self.options.partition
                        ),
                    )
                })?,
        );
        let start = match self.options.start_offset {
            KafkaStartOffset::Earliest => StartOffset::Earliest,
            KafkaStartOffset::Latest => StartOffset::Latest,
        };
        let mut stream = Box::pin(
            StreamConsumerBuilder::new(partition, start)
                .with_max_wait_ms(self.options.max_wait_ms.max(1))
                .build(),
        );

        loop {
            tokio::select! {
                _ = ct.cancelled() => return Ok(()),
                item = stream.next() => {
                    match item {
                        Some(Ok((record, _high_watermark))) => {
                            let Some(value) = record.record.value else {
                                continue;
                            };
                            match EventMessage::from_wire(&value) {
                                Ok(event) => {
                                    if let Err(error) = self.dispatcher.dispatch(&event).await {
                                        tracing::error!(
                                            event = %event.event_name,
                                            offset = record.offset,
                                            error = %error,
                                            "Kafka event handler failed"
                                        );
                                    }
                                }
                                Err(error) => {
                                    tracing::error!(
                                        offset = record.offset,
                                        error = %error,
                                        "Kafka event frame is invalid"
                                    );
                                }
                            }
                        }
                        Some(Err(error)) => {
                            return Err(Error::status(
                                503,
                                format!("Kafka fetch failed: {error}"),
                            ));
                        }
                        None => return Ok(()),
                    }
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl BackgroundService for KafkaEventConsumer {
    async fn execute(&self, ct: CancellationToken) -> Result<()> {
        let mut backoff = Duration::from_secs(1);
        loop {
            if ct.is_cancelled() {
                return Ok(());
            }
            // Cancellation must interrupt a slow connect/fetch, not just the
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
                        topic = %self.options.topic,
                        partition = self.options.partition,
                        error = %error,
                        "Kafka event consumer lost its partition; reconnecting"
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

async fn ensure_topic(client: &Client, options: &KafkaOptions) -> Result<()> {
    let topics = client
        .list_topics()
        .await
        .map_err(|error| Error::status(503, format!("Kafka metadata request failed: {error}")))?;
    if topics.iter().any(|topic| topic.name == options.topic) {
        return Ok(());
    }
    if !options.create_topic {
        return Err(Error::status(
            503,
            format!("Kafka topic '{}' does not exist", options.topic),
        ));
    }

    let controller = client
        .controller_client()
        .map_err(|error| Error::status(503, format!("Kafka controller lookup failed: {error}")))?;
    match controller
        .create_topic(
            options.topic.clone(),
            options.partitions.max(1),
            options.replication_factor.max(1),
            5_000,
        )
        .await
    {
        Ok(()) => Ok(()),
        // Another instance won the create race; the topic now exists.
        Err(rskafka::client::error::Error::ServerError {
            protocol_error: rskafka::client::error::ProtocolError::TopicAlreadyExists,
            ..
        }) => Ok(()),
        Err(error) => Err(Error::status(
            503,
            format!("Kafka topic creation failed: {error}"),
        )),
    }
}
