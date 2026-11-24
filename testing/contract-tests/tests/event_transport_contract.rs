//! Self-test for [`EventTransportContract`] using an in-process loopback.
//!
//! PR CI has no broker, so this proves the suite itself: the contract must pass
//! for a conforming transport and fail (with a diagnostic) for a broken one.
//! Real adapters run the same suite in the nightly broker job.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use webx_contract_tests::event_transport::EventTransportContract;
use webx_core::error::{Error, Result};
use webx_core::hosted::{BackgroundService, CancellationToken};
use webx_core::mediator::{EventDispatcher, EventMessage, IEventPublisher};
use webx_core::realtime::RealtimeFanoutEvent;

struct LoopbackPublisher {
    tx: mpsc::UnboundedSender<EventMessage>,
}

#[async_trait]
impl IEventPublisher for LoopbackPublisher {
    async fn publish(&self, message: EventMessage) -> Result<()> {
        self.tx
            .send(message)
            .map_err(|_| Error::Internal("loopback consumer is gone".to_string()))
    }
}

struct LoopbackConsumer {
    rx: Mutex<Option<mpsc::UnboundedReceiver<EventMessage>>>,
    dispatcher: Arc<EventDispatcher>,
}

#[async_trait]
impl BackgroundService for LoopbackConsumer {
    async fn execute(&self, ct: CancellationToken) -> Result<()> {
        let mut rx = self
            .rx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .expect("the loopback consumer runs once");
        loop {
            if ct.is_cancelled() {
                return Ok(());
            }
            match tokio::time::timeout(Duration::from_millis(20), rx.recv()).await {
                Ok(Some(message)) => self.dispatcher.dispatch(&message).await?,
                Ok(None) => return Ok(()),
                Err(_) => continue,
            }
        }
    }
}

struct BrokenPublisher;

#[async_trait]
impl IEventPublisher for BrokenPublisher {
    async fn publish(&self, _message: EventMessage) -> Result<()> {
        Err(Error::Status(503, "transport is down".to_string()))
    }
}

#[tokio::test]
async fn loopback_transport_satisfies_the_contract() {
    let (tx, rx) = mpsc::unbounded_channel();
    let contract = EventTransportContract::new();
    let consumer: Arc<dyn BackgroundService> = Arc::new(LoopbackConsumer {
        rx: Mutex::new(Some(rx)),
        dispatcher: contract.dispatcher(),
    });
    let report = contract
        .run(&LoopbackPublisher { tx }, consumer, Duration::from_secs(5))
        .await
        .expect("a conforming loopback transport must pass");

    assert!(report.delivered_count() >= 1);
    assert_eq!(
        report.event_name,
        RealtimeFanoutEvent::EVENT_NAME,
        "the transport must preserve the event name"
    );
    assert!(report
        .delivered
        .iter()
        .all(|event| event.room == "contract-tests-event-transport"
            && event.payload == "contract-tests-event-payload"));
}

#[tokio::test]
async fn a_broken_transport_fails_with_a_diagnostic() {
    let contract = EventTransportContract::new();
    let consumer: Arc<dyn BackgroundService> = Arc::new(LoopbackConsumer {
        rx: Mutex::new(Some(mpsc::unbounded_channel().1)),
        dispatcher: contract.dispatcher(),
    });
    let error = contract
        .run(&BrokenPublisher, consumer, Duration::from_millis(300))
        .await
        .expect_err("a transport that never delivers must fail the contract");
    let message = format!("{error}");
    assert!(
        message.contains("no delivery"),
        "unexpected contract error: {message}"
    );
}
