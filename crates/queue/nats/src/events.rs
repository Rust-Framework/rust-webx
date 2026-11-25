use std::sync::Arc;
use std::time::Duration;

use async_nats::Message;
use futures_util::{Stream, StreamExt};
use webx_core::error::{Error, Result};
use webx_core::hosted::{delay, BackgroundService, CancellationToken};
use webx_core::mediator::{EventDispatcher, EventMessage, IEventPublisher};

use crate::connection::NatsConnection;
use crate::options::{derived_broadcast_subject, NatsOptions};

/// NATS core publisher.
///
/// Core NATS is at-most-once: a message published while no subscriber is
/// connected is gone. Use Redis Streams or RabbitMQ when durability matters.
///
/// Competing events are published to `subject`; broadcast events (see
/// [`IEventPublisher::broadcast`]) are published to a separate subject
/// (`"<subject>.broadcast"` by default) that every node subscribes to without a
/// queue group.
pub struct NatsEventPublisher {
    connection: Arc<NatsConnection>,
    subject: String,
    broadcast_subject: String,
}

impl NatsEventPublisher {
    /// Create a publisher over a shared connection.
    ///
    /// Broadcast events are published to `"<subject>.broadcast"`; use
    /// `NatsOptions::with_broadcast_subject` overrides it through `add_nats`.
    pub fn new(connection: Arc<NatsConnection>, subject: impl Into<String>) -> Self {
        let subject = subject.into();
        let broadcast_subject = derived_broadcast_subject(&subject);
        Self {
            connection,
            subject,
            broadcast_subject,
        }
    }

    /// Override the broadcast subject (empty = derive `"<subject>.broadcast"`).
    pub(crate) fn with_broadcast_subject(mut self, subject: impl Into<String>) -> Self {
        let subject = subject.into();
        self.broadcast_subject = if subject.trim().is_empty() {
            derived_broadcast_subject(&self.subject)
        } else {
            subject
        };
        self
    }

    /// Subject competing events are published to (tests).
    #[cfg(test)]
    pub(crate) fn subject(&self) -> &str {
        &self.subject
    }

    /// Subject broadcast events are published to (tests).
    #[cfg(test)]
    pub(crate) fn broadcast_subject(&self) -> &str {
        &self.broadcast_subject
    }

    async fn publish_to(&self, subject: &str, message: EventMessage) -> Result<()> {
        let frame = message.to_wire()?;
        let client = self.connection.client().await?;
        client
            .publish(subject.to_string(), frame.into())
            .await
            .map_err(|error| Error::status(503, format!("NATS publish failed: {error}")))
    }
}

#[async_trait::async_trait]
impl IEventPublisher for NatsEventPublisher {
    async fn publish(&self, message: EventMessage) -> Result<()> {
        self.publish_to(&self.subject, message).await
    }

    /// Publish to the broadcast subject, which the consumer subscribes without a
    /// queue group. Every node receives its own copy; delivery is at-most-once.
    async fn broadcast(&self, message: EventMessage) -> Result<()> {
        self.publish_to(&self.broadcast_subject, message).await
    }
}

/// NATS core subscriber that dispatches events through [`EventDispatcher`].
///
/// Two independently reconnecting loops share the dispatcher: one consumes the
/// competing subject (`subject`, honouring `queue_group`) and one consumes the
/// broadcast subject (`NatsOptions::broadcast_subject` or `"<subject>.broadcast"`) **without** a
/// queue group, so every node sees broadcast events while competing events stay
/// load-balanced. Both stop with the [`CancellationToken`]; a broken broadcast
/// subscription is logged and retried without stopping competing delivery.
///
/// Delivery is at-most-once: handler failures and undecodable frames are logged
/// and dropped, never redelivered.
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
        let subscriber = match self.options.queue_group.as_deref() {
            Some(group) if !group.trim().is_empty() => {
                client
                    .queue_subscribe(subject.clone(), group.to_string())
                    .await
            }
            _ => client.subscribe(subject.clone()).await,
        }
        .map_err(|error| Error::status(503, format!("NATS subscribe failed: {error}")))?;
        self.consume(subscriber, &subject, ct).await
    }

    async fn run_broadcast_once(&self, ct: &CancellationToken) -> Result<()> {
        let client = self.connection.client().await?;
        let subject = self.options.effective_broadcast_subject();
        // Broadcast delivery never uses a queue group: every node gets a copy.
        let subscriber = client
            .subscribe(subject.clone())
            .await
            .map_err(|error| Error::status(503, format!("NATS subscribe failed: {error}")))?;
        self.consume(subscriber, &subject, ct).await
    }

    /// Drain one subscription; the caller reconnects when it ends.
    async fn consume<S>(
        &self,
        mut subscriber: S,
        subject: &str,
        ct: &CancellationToken,
    ) -> Result<()>
    where
        S: Stream<Item = Message> + Unpin + Send,
    {
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
                                    subject = %subject,
                                    event = %event.event_name,
                                    error = %error,
                                    "NATS event handler failed"
                                );
                            }
                        }
                        Err(error) => {
                            tracing::error!(
                                subject = %subject,
                                error = %error,
                                "NATS event frame is invalid"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Reconnect loop for the competing subscription (pre-2.9.2 behaviour).
    async fn competing_loop(&self, ct: CancellationToken) {
        let mut backoff = Duration::from_secs(1);
        loop {
            if ct.is_cancelled() {
                return;
            }
            // Cancellation must interrupt a slow connect/subscribe, not just
            // the idle wait between attempts.
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
                        subject = %self.options.subject,
                        error = %error,
                        "NATS event consumer lost its subscription; reconnecting"
                    );
                }
            }
            if !delay(&ct, backoff).await {
                return;
            }
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }

    /// Reconnect loop for the broadcast subscription. Errors are logged and
    /// retried here, so a broken broadcast subject never stops competing
    /// delivery.
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
                        subject = %self.options.effective_broadcast_subject(),
                        error = %error,
                        "NATS broadcast consumer lost its subscription; reconnecting"
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
impl BackgroundService for NatsEventConsumer {
    async fn execute(&self, ct: CancellationToken) -> Result<()> {
        // Both deliveries must run concurrently: a stalled broadcast
        // subscription must not stop competing consumption, and vice versa.
        tokio::join!(
            self.competing_loop(ct.clone()),
            self.broadcast_loop(ct.clone()),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_subject_defaults_to_a_derived_sibling() {
        let connection = Arc::new(NatsConnection::new(
            "nats://127.0.0.1:1",
            Duration::from_secs(1),
        ));
        let publisher = NatsEventPublisher::new(Arc::clone(&connection), "app.events");
        assert_eq!(publisher.subject(), "app.events");
        assert_eq!(publisher.broadcast_subject(), "app.events.broadcast");

        // An explicit value wins; a blank one falls back to the derivation.
        let publisher = publisher.with_broadcast_subject("app.broadcast");
        assert_eq!(publisher.broadcast_subject(), "app.broadcast");
        let publisher = publisher.with_broadcast_subject("  ");
        assert_eq!(publisher.broadcast_subject(), "app.events.broadcast");
    }

    #[test]
    fn broadcast_subject_matches_the_options_derivation() {
        let connection = Arc::new(NatsConnection::new(
            "nats://127.0.0.1:1",
            Duration::from_secs(1),
        ));
        let options = NatsOptions::new("nats://127.0.0.1:1").with_subject("webx.realtime");
        let publisher = NatsEventPublisher::new(Arc::clone(&connection), options.subject.clone());
        assert_eq!(
            publisher.broadcast_subject(),
            options.effective_broadcast_subject()
        );
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
}
