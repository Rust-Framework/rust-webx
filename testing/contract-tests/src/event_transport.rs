//! Event transport contract (2.5 follow-up to 2.4-7).
//!
//! Every event-transport adapter (in-memory, Redis, NATS, RabbitMQ, Kafka)
//! must move an [`EventMessage`] from a publisher to the framework's
//! [`EventDispatcher`] with the event name and JSON payload intact. This suite
//! proves exactly that against a real adapter:
//!
//! ```ignore
//! #[tokio::test]
//! async fn redis_transport_satisfies_the_contract() {
//!     let Ok(url) = std::env::var("WEBX_REDIS_URL") else { return };
//!     let contract = EventTransportContract::new();
//!     let connection = Arc::new(RedisConnection::new(url, timeout, timeout));
//!     let options = RedisEventsOptions::default();
//!     let publisher = RedisEventPublisher::new(Arc::clone(&connection), options.clone());
//!     let consumer = Arc::new(RedisEventConsumer::new(
//!         connection,
//!         options,
//!         contract.dispatcher(),
//!     ));
//!     let report = contract
//!         .run(&publisher, consumer, Duration::from_secs(10))
//!         .await
//!         .expect("redis event transport");
//!     assert!(report.delivered_count() >= 1);
//! }
//! ```
//!
//! The suite deliberately does not assert delivery count: adapters are allowed
//! to be at-most-once (in-memory) or at-least-once (consumer groups with
//! retry). It asserts that at least one delivery arrives and that every
//! delivery carries the same room/payload as the published event.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rust_dix::ServiceCollection;
use webx_core::error::{Error, Result};
use webx_core::handler::IEventHandler;
use webx_core::hosted::{BackgroundService, CancellationToken};
use webx_core::mediator::{EventDispatcher, EventMessage, IEventPublisher};
use webx_core::realtime::RealtimeFanoutEvent;

/// Contract event used by the suite.
///
/// The framework's `RealtimeFanoutEvent` is used on purpose: its inventory
/// registration ships with `rust-webx-core`, so the suite does not have to add
/// its own global event registration to every adapter test binary.
const ROOM: &str = "contract-tests-event-transport";
const PAYLOAD: &str = "contract-tests-event-payload";

/// Result of one contract run.
#[derive(Debug, Clone)]
pub struct EventTransportReport {
    /// Event name that was published.
    pub event_name: String,
    /// Every delivery observed by the sink handler (duplicates are possible for
    /// at-least-once adapters).
    pub delivered: Vec<RealtimeFanoutEvent>,
}

impl EventTransportReport {
    /// Number of deliveries that reached the framework dispatcher.
    pub fn delivered_count(&self) -> usize {
        self.delivered.len()
    }
}

struct ContractSink {
    seen: Arc<Mutex<Vec<RealtimeFanoutEvent>>>,
}

#[async_trait]
impl IEventHandler<RealtimeFanoutEvent> for ContractSink {
    async fn handle(&self, event: RealtimeFanoutEvent) -> Result<()> {
        self.seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(event);
        Ok(())
    }
}

/// A two-phase harness so adapters can build their consumer with the same
/// dispatcher the suite will observe.
pub struct EventTransportContract {
    dispatcher: Arc<EventDispatcher>,
    seen: Arc<Mutex<Vec<RealtimeFanoutEvent>>>,
}

impl EventTransportContract {
    /// Build the sink handler + dispatcher used by the suite.
    ///
    /// # Panics
    ///
    /// Panics if the minimal test service provider cannot be built (a
    /// construction bug in the contract crate itself).
    pub fn new() -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        let provider = ServiceCollection::new()
            .singleton::<dyn IEventHandler<RealtimeFanoutEvent>>(move |_| {
                Arc::new(ContractSink {
                    seen: Arc::clone(&sink_seen),
                })
            })
            .build()
            .expect("contract test provider must build");
        Self {
            dispatcher: Arc::new(EventDispatcher::new(provider)),
            seen,
        }
    }

    /// Dispatcher the consumer must deliver through.
    pub fn dispatcher(&self) -> Arc<EventDispatcher> {
        Arc::clone(&self.dispatcher)
    }

    /// Publish one event through `publisher`, wait for the sink, stop
    /// `consumer`, and return every observed delivery.
    ///
    /// Publishing is retried until the sink observes an event (core NATS drops
    /// messages published before the subscriber registers; Redis/Kafka may need
    /// their consumer group/partition assigned first), until `timeout` expires.
    pub async fn run(
        &self,
        publisher: &dyn IEventPublisher,
        consumer: Arc<dyn BackgroundService>,
        timeout: Duration,
    ) -> Result<EventTransportReport> {
        let expected = RealtimeFanoutEvent::to_room(ROOM, PAYLOAD);
        let message = EventMessage::from_event(&expected)?;
        if !self.dispatcher.has_event(&message.event_name) {
            return Err(Error::Di(format!(
                "event transport contract: no local registration for `{}` \
                 (is rust-webx-core linked into this test binary?)",
                message.event_name
            )));
        }

        let ct = CancellationToken::new();
        let consumer_ct = ct.clone();
        let mut task = tokio::spawn(async move { consumer.execute(consumer_ct).await });

        let deadline = Instant::now() + timeout;
        let mut publish_error = None;
        loop {
            let delivered = self
                .seen
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len();
            if delivered > 0 || Instant::now() >= deadline {
                break;
            }
            if let Err(error) = publisher.publish(message.clone()).await {
                publish_error = Some(error);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        ct.cancel();
        match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => {
                return Err(Error::Internal(format!(
                    "event transport contract: consumer failed: {error}"
                )))
            }
            Ok(Err(error)) => {
                return Err(Error::Internal(format!(
                    "event transport contract: consumer task panicked: {error}"
                )))
            }
            Err(_) => {
                task.abort();
                let _ = task.await;
                return Err(Error::Internal(
                    "event transport contract: consumer did not stop within 5s".to_string(),
                ));
            }
        }

        let delivered = self
            .seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if delivered.is_empty() {
            let detail = publish_error
                .map(|error| format!(" (last publish error: {error})"))
                .unwrap_or_default();
            return Err(Error::Internal(format!(
                "event transport contract: no delivery within {timeout:?}{detail}"
            )));
        }
        for event in &delivered {
            assert_eq!(
                event.room, ROOM,
                "delivered event room must match the published event"
            );
            assert_eq!(
                event.payload, PAYLOAD,
                "delivered event payload must match the published event"
            );
        }
        Ok(EventTransportReport {
            event_name: message.event_name,
            delivered,
        })
    }
}

impl Default for EventTransportContract {
    fn default() -> Self {
        Self::new()
    }
}
