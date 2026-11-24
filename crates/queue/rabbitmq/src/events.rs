use std::sync::Arc;
use std::time::Duration;

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
}

#[async_trait::async_trait]
impl IEventPublisher for RabbitMqEventPublisher {
    async fn publish(&self, message: EventMessage) -> Result<()> {
        let frame = message.to_wire()?;
        let channel = self.channel().await?;
        let confirmation = channel
            .basic_publish(
                self.options.exchange.clone().into(),
                self.options.routing_key.clone().into(),
                BasicPublishOptions {
                    // Unroutable events must surface as errors instead of an
                    // implicit drop with an "acked" publisher confirm.
                    mandatory: true,
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

/// RabbitMQ consumer with manual acks and requeue-on-handler-failure.
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
}

#[async_trait::async_trait]
impl BackgroundService for RabbitMqEventConsumer {
    async fn execute(&self, ct: CancellationToken) -> Result<()> {
        let mut backoff = Duration::from_secs(1);
        loop {
            if ct.is_cancelled() {
                return Ok(());
            }
            // Cancellation must interrupt a slow connect/consume, not just the
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
                        queue = %self.options.queue,
                        error = %error,
                        "RabbitMQ event consumer lost its connection; reconnecting"
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
}
