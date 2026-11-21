use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use webx_core::error::{Error, Result};
use webx_core::hosted::{delay, BackgroundService, CancellationToken};
use webx_core::mediator::{EventDispatcher, EventMessage, IEventPublisher};

use crate::connection::NatsConnection;
use crate::options::NatsOptions;

/// NATS core publisher.
///
/// Core NATS is at-most-once: a message published while no subscriber is
/// connected is gone. Use Redis Streams or RabbitMQ when durability matters.
pub struct NatsEventPublisher {
    connection: Arc<NatsConnection>,
    subject: String,
}

impl NatsEventPublisher {
    /// Create a publisher over a shared connection.
    pub fn new(connection: Arc<NatsConnection>, subject: impl Into<String>) -> Self {
        Self {
            connection,
            subject: subject.into(),
        }
    }
}

#[async_trait::async_trait]
impl IEventPublisher for NatsEventPublisher {
    async fn publish(&self, message: EventMessage) -> Result<()> {
        let frame = message.to_wire()?;
        let client = self.connection.client().await?;
        client
            .publish(self.subject.clone(), frame.into())
            .await
            .map_err(|error| Error::Status(503, format!("NATS publish failed: {error}")))
    }
}

/// NATS core subscriber that dispatches events through [`EventDispatcher`].
///
/// A queue group makes several instances share the stream instead of each
/// receiving a copy. Delivery is at-most-once; handler failures are logged.
pub struct NatsEventConsumer {
    connection: Arc<NatsConnection>,
    options: NatsOptions,
    dispatcher: Arc<EventDispatcher>,
}

impl NatsEventConsumer {
    /// Create a consumer worker over a shared connection and dispatcher.
    pub fn new(
        connection: Arc<NatsConnection>,
        options: NatsOptions,
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
        let subject = self.options.subject.clone();
        let mut subscriber = match self.options.queue_group.as_deref() {
            Some(group) if !group.trim().is_empty() => {
                client.queue_subscribe(subject, group.to_string()).await
            }
            _ => client.subscribe(subject).await,
        }
        .map_err(|error| Error::Status(503, format!("NATS subscribe failed: {error}")))?;

        loop {
            tokio::select! {
                _ = ct.cancelled() => return Ok(()),
                message = subscriber.next() => {
                    let Some(message) = message else {
                        return Ok(());
                    };
                    match EventMessage::from_wire(&message.payload) {
                        Ok(event) => {
                            if let Err(error) = self.dispatcher.dispatch(&event).await {
                                tracing::error!(
                                    event = %event.event_name,
                                    error = %error,
                                    "NATS event handler failed"
                                );
                            }
                        }
                        Err(error) => {
                            tracing::error!(error = %error, "NATS event frame is invalid");
                        }
                    }
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl BackgroundService for NatsEventConsumer {
    async fn execute(&self, ct: CancellationToken) -> Result<()> {
        let mut backoff = Duration::from_secs(1);
        loop {
            if ct.is_cancelled() {
                return Ok(());
            }
            // Cancellation must interrupt a slow connect/subscribe, not just
            // the idle wait between attempts.
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
                        subject = %self.options.subject,
                        error = %error,
                        "NATS event consumer lost its subscription; reconnecting"
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
