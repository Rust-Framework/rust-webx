//! Live RabbitMQ integration test for the event transport.
//!
//! Set `WEBX_RABBITMQ_URL` (for example
//! `amqp://guest:guest@127.0.0.1:5672/%2f`) to run it. Without a broker the
//! test returns early and passes.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rust_dix::{ScopeFactory, ServiceCollection, ServiceProvider};
use webx_core::handler::IEventHandler;
use webx_core::hosted::{BackgroundService, CancellationToken};
use webx_core::mediator::{
    dispatch_event_handlers, BoxedEventDispatch, EventDispatcher, EventMessage, EventRegistration,
    IEventPublisher, IEventRequest,
};
use webx_rabbitmq::{
    RabbitConnection, RabbitMqEventConsumer, RabbitMqEventPublisher, RabbitMqOptions,
};

use webx_contract_tests::event_transport::EventTransportContract;
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct TestEvent {
    value: u32,
}

impl IEventRequest for TestEvent {}

/// Manual equivalent of the `#[handler]`-generated registration: adapters do
/// not depend on the `webx` umbrella crate, so the inventory entry is written
/// by hand here.
fn dispatch_test_event(provider: Arc<ServiceProvider>, payload: Vec<u8>) -> BoxedEventDispatch {
    Box::pin(async move {
        let message = EventMessage::new(<TestEvent as IEventRequest>::event_name(), payload);
        let event = message.decode::<TestEvent>()?;
        let scope = <ServiceProvider as ScopeFactory>::create_scope(provider.as_ref());
        dispatch_event_handlers(&scope, event).await
    })
}

inventory::submit! {
    EventRegistration {
        event_name: <TestEvent as IEventRequest>::event_name,
        event_type_id: std::any::TypeId::of::<TestEvent>(),
        dispatch: dispatch_test_event,
    }
}

struct CountingHandler {
    counted: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl IEventHandler<TestEvent> for CountingHandler {
    async fn handle(&self, event: TestEvent) -> webx_core::error::Result<()> {
        self.counted.fetch_add(event.value, Ordering::SeqCst);
        Ok(())
    }
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("{}-{nanos}", std::process::id())
}

fn provider_with_counting_handler(counted: Arc<AtomicU32>) -> Arc<ServiceProvider> {
    ServiceCollection::new()
        .singleton::<dyn IEventHandler<TestEvent>>(move |_| {
            Arc::new(CountingHandler {
                counted: Arc::clone(&counted),
            })
        })
        .build()
        .expect("test service provider must build")
}

#[tokio::test]
async fn event_roundtrip() {
    let Ok(url) = std::env::var("WEBX_RABBITMQ_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let topology = (
        format!("webx.test.exchange.{suffix}"),
        format!("webx.test.routing.{suffix}"),
        format!("webx.test.queue.{suffix}"),
    );
    let counted = Arc::new(AtomicU32::new(0));
    let provider = provider_with_counting_handler(Arc::clone(&counted));
    let dispatcher = Arc::new(EventDispatcher::new(provider));

    let connection = Arc::new(RabbitConnection::new(url.clone(), Duration::from_secs(5)));
    let options = RabbitMqOptions::new(url).with_topology(topology.0, topology.1, topology.2);
    let publisher = RabbitMqEventPublisher::new(Arc::clone(&connection), options.clone());
    let consumer =
        RabbitMqEventConsumer::new(Arc::clone(&connection), options, Arc::clone(&dispatcher));

    let ct = CancellationToken::new();
    let consumer_ct = ct.clone();
    let mut task = tokio::spawn(async move { consumer.execute(consumer_ct).await });

    // Re-publish until the handler observes an event, so the test does not
    // depend on the consumer having finished its queue declaration/bind.
    let message = EventMessage::from_event(&TestEvent { value: 1 }).expect("encode event");
    let mut publish_error = None;
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while counted.load(Ordering::SeqCst) == 0 {
            if let Err(error) = publisher.publish(message.clone()).await {
                publish_error = Some(error);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    ct.cancel();
    match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => panic!("rabbitmq consumer failed: {error}"),
        Ok(Err(error)) => panic!("rabbitmq consumer task panicked: {error}"),
        Err(_) => {
            task.abort();
            let _ = task.await;
            panic!("rabbitmq consumer did not stop within 5s of cancellation");
        }
    }

    if let Some(error) = publish_error {
        panic!("publishing the test event failed: {error}");
    }
    assert!(
        counted.load(Ordering::SeqCst) > 0,
        "the event handler did not receive the published event"
    );
}

#[tokio::test]
async fn event_transport_satisfies_the_shared_contract() {
    let Ok(url) = std::env::var("WEBX_RABBITMQ_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let contract = EventTransportContract::new();
    let connection = Arc::new(RabbitConnection::new(url.clone(), Duration::from_secs(5)));
    let options = RabbitMqOptions::new(url).with_topology(
        format!("webx.contract.exchange.{suffix}"),
        format!("webx.contract.routing.{suffix}"),
        format!("webx.contract.queue.{suffix}"),
    );
    let publisher = RabbitMqEventPublisher::new(Arc::clone(&connection), options.clone());
    let consumer: Arc<dyn BackgroundService> = Arc::new(RabbitMqEventConsumer::new(
        Arc::clone(&connection),
        options,
        contract.dispatcher(),
    ));
    let report = contract
        .run(&publisher, consumer, Duration::from_secs(10))
        .await
        .expect("rabbitmq event transport contract");
    assert!(report.delivered_count() >= 1);
}
