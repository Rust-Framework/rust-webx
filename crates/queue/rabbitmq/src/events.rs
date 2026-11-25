use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use lapin::message::Delivery;
use lapin::options::{
    BasicAckOptions, BasicConsumeOptions, BasicNackOptions, BasicPublishOptions, BasicQosOptions,
    ExchangeDeclareOptions, QueueBindOptions, QueueDeclareOptions,
};
use lapin::types::{AMQPValue, FieldTable};
use lapin::{BasicProperties, Channel, Confirmation, Connection, ExchangeKind};
use tokio::sync::Mutex;
use webx_core::error::{Error, Result};
use webx_core::hosted::{delay, BackgroundService, CancellationToken};
use webx_core::mediator::{EventDispatcher, EventMessage, IEventPublisher};

use crate::connection::RabbitConnection;
use crate::options::RabbitMqOptions;

struct CachedChannel {
    connection: Arc<Connection>,
    channel: Channel,
}

/// RabbitMQ publisher with publisher confirms and a durable topic exchange.
///
/// `publish` returns after the broker confirms the message; handler execution
/// happens in [`RabbitMqEventConsumer`] (or another service sharing the queue).
/// A broker-side unroutable message (`NO_ROUTE`, e.g. while a fresh deployment
/// is still creating/ binding the queue) is reported as a retryable `503`; a
/// publisher that must not lose the event retries with backoff.
/// Broadcast events (`IEventPublisher::broadcast`) are published to a fanout
/// exchange (`"<exchange>.broadcast"` by default) where every node binds its own
/// exclusive auto-delete queue, so each node receives one copy.
pub struct RabbitMqEventPublisher {
    connection: Arc<RabbitConnection>,
    options: RabbitMqOptions,
    channel: Mutex<Option<CachedChannel>>,
}

impl RabbitMqEventPublisher {
    /// Create a publisher over a shared connection.
    pub fn new(connection: Arc<RabbitConnection>, options: RabbitMqOptions) -> Self {
        Self {
            connection,
            options,
            channel: Mutex::new(None),
        }
    }

    async fn channel(&self) -> Result<Channel> {
        let connection = self.connection.connection().await?;
        let mut slot = self.channel.lock().await;
        if let Some(cached) = slot.as_ref() {
            if Arc::ptr_eq(&cached.connection, &connection) && cached.channel.status().connected() {
                return Ok(cached.channel.clone());
            }
        }

        let channel = connection.create_channel().await.map_err(|error| {
            Error::status(503, format!("RabbitMQ channel open failed: {error}"))
        })?;
        ensure_topology(&channel, &self.options).await?;
        ensure_broadcast_exchange(&channel, &self.options).await?;

        // Enable publisher confirms so `publish` observes broker acceptance.
        channel
            .confirm_select(lapin::options::ConfirmSelectOptions::default())
            .await
            .map_err(|error| {
                Error::status(503, format!("RabbitMQ confirm.select failed: {error}"))
            })?;

        *slot = Some(CachedChannel {
            connection,
            channel: channel.clone(),
        });
        Ok(channel)
    }

    async fn publish_to(
        &self,
        exchange: String,
        routing_key: String,
        mandatory: bool,
        message: EventMessage,
    ) -> Result<()> {
        let frame = message.to_wire()?;
        let channel = self.channel().await?;
        let confirmation = channel
            .basic_publish(
                exchange.into(),
                routing_key.into(),
                BasicPublishOptions {
                    // Unroutable competing events must surface as errors instead
                    // of an implicit drop with an "acked" publisher confirm.
                    mandatory,
                    ..Default::default()
                },
                &frame,
                BasicProperties::default().with_delivery_mode(2),
            )
            .await
            .map_err(|error| Error::status(503, format!("RabbitMQ publish failed: {error}")))?
            .await
            .map_err(|error| {
                Error::status(503, format!("RabbitMQ publisher confirm failed: {error}"))
            })?;

        match confirmation {
            Confirmation::Ack(None) => Ok(()),
            Confirmation::Ack(Some(returned)) => Err(Error::status(
                503,
                format!(
                    "RabbitMQ could not route the event: {}",
                    returned.reply_text
                ),
            )),
            Confirmation::Nack(_) => Err(Error::status(
                503,
                "RabbitMQ rejected the event".to_string(),
            )),
            Confirmation::NotRequested => Err(Error::status(
                503,
                "RabbitMQ publisher confirms are not enabled".to_string(),
            )),
        }
    }
}

#[async_trait::async_trait]
impl IEventPublisher for RabbitMqEventPublisher {
    async fn publish(&self, message: EventMessage) -> Result<()> {
        self.publish_to(
            self.options.exchange.clone(),
            self.options.routing_key.clone(),
            true,
            message,
        )
        .await
    }

    /// Publish to the broadcast fanout exchange.
    ///
    /// Delivery is at-most-once: when no node has a queue bound, RabbitMQ
    /// accepts and drops the event (it is not an error — a broadcast has no
    /// "the one consumer" to route to).
    async fn broadcast(&self, message: EventMessage) -> Result<()> {
        self.publish_to(
            self.options.effective_broadcast_exchange(),
            String::new(),
            // No queue bound is a valid broadcast state (no node listening).
            false,
            message,
        )
        .await
    }
}

/// RabbitMQ consumer with manual acks and requeue-on-handler-failure.
///
/// Two independent loops share one [`EventDispatcher`]: the competing loop
/// consumes the durable quorum queue (unchanged semantics), and the broadcast
/// loop consumes a per-process exclusive auto-delete queue bound to the fanout
/// exchange, so every node receives its own copy of a broadcast event.
pub struct RabbitMqEventConsumer {
    connection: Arc<RabbitConnection>,
    options: RabbitMqOptions,
    dispatcher: Arc<EventDispatcher>,
}

impl RabbitMqEventConsumer {
    /// Create a consumer worker over a shared connection and dispatcher.
    pub fn new(
        connection: Arc<RabbitConnection>,
        options: RabbitMqOptions,
        dispatcher: Arc<EventDispatcher>,
    ) -> Self {
        Self {
            connection,
            options,
            dispatcher,
        }
    }

    async fn run_once(&self, ct: &CancellationToken) -> Result<()> {
        let connection = self.connection.connection().await?;
        let channel = connection.create_channel().await.map_err(|error| {
            Error::status(503, format!("RabbitMQ channel open failed: {error}"))
        })?;
        channel
            .basic_qos(self.options.prefetch.max(1), BasicQosOptions::default())
            .await
            .map_err(|error| Error::status(503, format!("RabbitMQ basic.qos failed: {error}")))?;
        ensure_topology(&channel, &self.options).await?;

        let mut consumer = channel
            .basic_consume(
                self.options.queue.clone().into(),
                "".into(),
                BasicConsumeOptions::default(),
                FieldTable::default(),
            )
            .await
            .map_err(|error| {
                Error::status(503, format!("RabbitMQ basic.consume failed: {error}"))
            })?;

        loop {
            tokio::select! {
                _ = ct.cancelled() => return Ok(()),
                delivery = consumer.next() => {
                    match delivery {
                        Some(Ok(delivery)) => self.handle(delivery).await?,
                        Some(Err(error)) => {
                            return Err(Error::status(
                                503,
                                format!("RabbitMQ consumer error: {error}"),
                            ));
                        }
                        None => return Ok(()),
                    }
                }
            }
        }
    }

    async fn handle(&self, delivery: Delivery) -> Result<()> {
        let event = match EventMessage::from_wire(&delivery.data) {
            Ok(event) => event,
            Err(error) => {
                // A malformed frame can never succeed on redelivery; ack it so
                // one poison message cannot block the queue forever.
                tracing::error!(error = %error, "RabbitMQ event frame is invalid; acking");
                delivery
                    .acker
                    .ack(BasicAckOptions::default())
                    .await
                    .map_err(|error| {
                        Error::status(503, format!("RabbitMQ basic.ack failed: {error}"))
                    })?;
                return Ok(());
            }
        };

        match self.dispatcher.dispatch(&event).await {
            Ok(()) => {
                delivery
                    .acker
                    .ack(BasicAckOptions::default())
                    .await
                    .map_err(|error| {
                        Error::status(503, format!("RabbitMQ basic.ack failed: {error}"))
                    })?;
            }
            Err(error) => {
                tracing::error!(
                    event = %event.event_name,
                    error = %error,
                    "RabbitMQ event handler failed; requeueing"
                );
                delivery
                    .acker
                    .nack(BasicNackOptions {
                        requeue: true,
                        ..Default::default()
                    })
                    .await
                    .map_err(|error| {
                        Error::status(503, format!("RabbitMQ basic.nack failed: {error}"))
                    })?;
                // Avoid a tight redelivery loop for a persistently failing
                // handler; the message is still at-least-once.
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        Ok(())
    }

    /// Declare the broadcast topology (fanout exchange + this process's
    /// exclusive auto-delete queue), then consume and dispatch broadcast events.
    async fn run_broadcast_once(&self, ct: &CancellationToken) -> Result<()> {
        let connection = self.connection.connection().await?;
        let channel = connection.create_channel().await.map_err(|error| {
            Error::status(503, format!("RabbitMQ channel open failed: {error}"))
        })?;
        channel
            .basic_qos(self.options.prefetch.max(1), BasicQosOptions::default())
            .await
            .map_err(|error| Error::status(503, format!("RabbitMQ basic.qos failed: {error}")))?;
        ensure_broadcast_exchange(&channel, &self.options).await?;

        // `exclusive + auto_delete` gives every node its own queue, so a
        // broadcast event is copied to each node instead of being load-balanced.
        let queue = broadcast_queue_name();
        channel
            .queue_declare(
                queue.clone().into(),
                broadcast_queue_options(),
                FieldTable::default(),
            )
            .await
            .map_err(|error| {
                Error::status(503, format!("RabbitMQ queue.declare failed: {error}"))
            })?;
        channel
            .queue_bind(
                queue.clone().into(),
                self.options.effective_broadcast_exchange().into(),
                "".into(),
                QueueBindOptions::default(),
                FieldTable::default(),
            )
            .await
            .map_err(|error| Error::status(503, format!("RabbitMQ queue.bind failed: {error}")))?;

        let mut consumer = channel
            .basic_consume(
                queue.into(),
                "".into(),
                BasicConsumeOptions::default(),
                FieldTable::default(),
            )
            .await
            .map_err(|error| {
                Error::status(503, format!("RabbitMQ basic.consume failed: {error}"))
            })?;

        loop {
            tokio::select! {
                _ = ct.cancelled() => return Ok(()),
                delivery = consumer.next() => {
                    match delivery {
                        Some(Ok(delivery)) => self.handle_broadcast(delivery).await?,
                        Some(Err(error)) => {
                            return Err(Error::status(
                                503,
                                format!("RabbitMQ broadcast consumer error: {error}"),
                            ));
                        }
                        None => return Ok(()),
                    }
                }
            }
        }
    }

    /// Broadcast is at-most-once: a failing handler is logged, never requeued
    /// (redelivery would only reach this same node).
    async fn handle_broadcast(&self, delivery: Delivery) -> Result<()> {
        let event = match EventMessage::from_wire(&delivery.data) {
            Ok(event) => event,
            Err(error) => {
                tracing::error!(error = %error, "RabbitMQ broadcast event frame is invalid; acking");
                return ack(&delivery).await;
            }
        };

        if let Err(error) = self.dispatcher.dispatch(&event).await {
            tracing::error!(
                event = %event.event_name,
                error = %error,
                "RabbitMQ broadcast event handler failed; not requeueing"
            );
        }
        ack(&delivery).await
    }

    /// Reconnect loop for the competing quorum queue (pre-2.9.2 behaviour).
    async fn competing_loop(&self, ct: CancellationToken) {
        let mut backoff = Duration::from_secs(1);
        loop {
            if ct.is_cancelled() {
                return;
            }
            // Cancellation must interrupt a slow connect/consume, not just the
            // idle wait between attempts.
            let result = tokio::select! {
                _ = ct.cancelled() => return,
                result = self.run_once(&ct) => result,
            };
            match result {
                Ok(()) if ct.is_cancelled() => return,
                Ok(()) => backoff = Duration::from_secs(1),
                Err(error) => {
                    if let Some(metrics) = self.dispatcher.metrics() {
                        metrics.record_consumer_reconnect();
                    }
                    tracing::warn!(
                        queue = %self.options.queue,
                        error = %error,
                        "RabbitMQ event consumer lost its connection; reconnecting"
                    );
                }
            }
            if !delay(&ct, backoff).await {
                return;
            }
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }

    /// Reconnect loop for this node's exclusive broadcast queue.
    async fn broadcast_loop(&self, ct: CancellationToken) {
        let mut backoff = Duration::from_secs(1);
        loop {
            if ct.is_cancelled() {
                return;
            }
            let result = tokio::select! {
                _ = ct.cancelled() => return,
                result = self.run_broadcast_once(&ct) => result,
            };
            match result {
                Ok(()) if ct.is_cancelled() => return,
                Ok(()) => backoff = Duration::from_secs(1),
                Err(error) => {
                    if let Some(metrics) = self.dispatcher.metrics() {
                        metrics.record_consumer_reconnect();
                    }
                    tracing::warn!(
                        exchange = %self.options.effective_broadcast_exchange(),
                        error = %error,
                        "RabbitMQ broadcast consumer lost its subscription; reconnecting"
                    );
                }
            }
            if !delay(&ct, backoff).await {
                return;
            }
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }
}

#[async_trait::async_trait]
impl BackgroundService for RabbitMqEventConsumer {
    async fn execute(&self, ct: CancellationToken) -> Result<()> {
        // Both deliveries must run concurrently: a stalled broadcast queue must
        // not stop competing consumption, and vice versa.
        tokio::join!(
            self.competing_loop(ct.clone()),
            self.broadcast_loop(ct.clone()),
        );
        Ok(())
    }
}

/// Quorum queues enforce `x-delivery-limit` in the broker: a handler that keeps
/// failing is retried a bounded number of times, then dead-lettered (when
/// configured) or dropped. This prevents a poison event from looping forever
/// without moving the retry/backoff policy into core.
fn queue_arguments(options: &RabbitMqOptions) -> FieldTable {
    let mut arguments = FieldTable::default();
    arguments.insert(
        "x-queue-type".into(),
        AMQPValue::LongString("quorum".into()),
    );
    arguments.insert(
        "x-delivery-limit".into(),
        AMQPValue::LongUInt(options.max_deliveries.max(1)),
    );
    if !options.dead_letter_exchange.trim().is_empty() {
        arguments.insert(
            "x-dead-letter-exchange".into(),
            AMQPValue::LongString(options.dead_letter_exchange.clone().into()),
        );
        arguments.insert(
            "x-dead-letter-routing-key".into(),
            AMQPValue::LongString(options.routing_key.clone().into()),
        );
    }
    arguments
}

async fn ensure_topology(channel: &Channel, options: &RabbitMqOptions) -> Result<()> {
    channel
        .exchange_declare(
            options.exchange.clone().into(),
            ExchangeKind::Topic,
            ExchangeDeclareOptions {
                durable: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await
        .map_err(|error| {
            Error::status(503, format!("RabbitMQ exchange.declare failed: {error}"))
        })?;

    channel
        .queue_declare(
            options.queue.clone().into(),
            QueueDeclareOptions {
                durable: true,
                ..Default::default()
            },
            queue_arguments(options),
        )
        .await
        .map_err(|error| Error::status(503, format!("RabbitMQ queue.declare failed: {error}")))?;

    channel
        .queue_bind(
            options.queue.clone().into(),
            options.exchange.clone().into(),
            options.routing_key.clone().into(),
            QueueBindOptions::default(),
            FieldTable::default(),
        )
        .await
        .map_err(|error| Error::status(503, format!("RabbitMQ queue.bind failed: {error}")))?;

    Ok(())
}

/// Declare the durable fanout exchange used for broadcast delivery.
async fn ensure_broadcast_exchange(channel: &Channel, options: &RabbitMqOptions) -> Result<()> {
    channel
        .exchange_declare(
            options.effective_broadcast_exchange().into(),
            ExchangeKind::Fanout,
            ExchangeDeclareOptions {
                durable: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await
        .map_err(|error| Error::status(503, format!("RabbitMQ exchange.declare failed: {error}")))
}

/// Per-process broadcast queue declaration: exclusive and auto-delete, so
/// RabbitMQ removes it when the connection closes and no two nodes can ever
/// share one broadcast queue.
fn broadcast_queue_options() -> QueueDeclareOptions {
    QueueDeclareOptions {
        durable: false,
        exclusive: true,
        auto_delete: true,
        ..Default::default()
    }
}

/// Unique broadcast queue name for this process (pid + time + sequence).
fn broadcast_queue_name() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!(
        "webx.broadcast.{}.{nanos}.{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

/// Acknowledge one delivery.
async fn ack(delivery: &Delivery) -> Result<()> {
    delivery
        .acker
        .ack(BasicAckOptions::default())
        .await
        .map(|_| ())
        .map_err(|error| Error::status(503, format!("RabbitMQ basic.ack failed: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argument<'a>(arguments: &'a FieldTable, key: &str) -> Option<&'a AMQPValue> {
        arguments
            .inner()
            .iter()
            .find(|(name, _)| name.as_str() == key)
            .map(|(_, value)| value)
    }

    #[test]
    fn quorum_queue_arguments_include_delivery_limit_and_dlx() {
        let mut options = RabbitMqOptions::new("amqp://rabbit");
        options.max_deliveries = 7;
        options.dead_letter_exchange = "app.events.dlx".to_string();

        let arguments = queue_arguments(&options);
        assert_eq!(
            argument(&arguments, "x-queue-type"),
            Some(&AMQPValue::LongString("quorum".into()))
        );
        assert_eq!(
            argument(&arguments, "x-delivery-limit"),
            Some(&AMQPValue::LongUInt(7))
        );
        assert_eq!(
            argument(&arguments, "x-dead-letter-exchange"),
            Some(&AMQPValue::LongString("app.events.dlx".into()))
        );
    }

    #[test]
    fn no_dead_letter_arguments_without_a_dlx() {
        let options = RabbitMqOptions::new("amqp://rabbit");
        let arguments = queue_arguments(&options);
        assert!(argument(&arguments, "x-dead-letter-exchange").is_none());
        assert_eq!(
            argument(&arguments, "x-delivery-limit"),
            Some(&AMQPValue::LongUInt(5))
        );
    }

    #[test]
    fn broadcast_queue_is_exclusive_and_auto_deleted() {
        let options = broadcast_queue_options();
        assert!(options.exclusive, "each node must own its queue");
        assert!(
            options.auto_delete,
            "the queue must disappear on disconnect"
        );
        assert!(!options.durable);
    }

    #[test]
    fn broadcast_queue_names_are_unique_per_process() {
        let first = broadcast_queue_name();
        let second = broadcast_queue_name();
        assert_ne!(first, second);
        assert!(first.starts_with("webx.broadcast."), "{first}");
        assert!(first.contains(&std::process::id().to_string()), "{first}");
    }

    #[test]
    fn broadcast_frames_round_trip_through_the_wire_format() {
        let message = EventMessage::new(
            "webx.realtime.user-fanout",
            br#"{"user_id":"u1","payload":"hello"}"#.to_vec(),
        );
        let wire = message.to_wire().expect("encode broadcast frame");
        assert_eq!(
            EventMessage::from_wire(&wire).expect("decode broadcast frame"),
            message
        );
    }
}
