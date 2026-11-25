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

/// Spawn a consumer (competing + broadcast loops) with its own cancellation token.
fn spawn_consumer(
    connection: &Arc<RabbitConnection>,
    options: RabbitMqOptions,
    dispatcher: Arc<EventDispatcher>,
) -> (
    CancellationToken,
    tokio::task::JoinHandle<webx_core::error::Result<()>>,
) {
    let consumer = RabbitMqEventConsumer::new(Arc::clone(connection), options, dispatcher);
    let ct = CancellationToken::new();
    let consumer_ct = ct.clone();
    let task = tokio::spawn(async move { consumer.execute(consumer_ct).await });
    (ct, task)
}

/// Cancel a consumer and fail the test if it does not stop within 5s.
async fn stop_consumer(
    ct: CancellationToken,
    mut task: tokio::task::JoinHandle<webx_core::error::Result<()>>,
) {
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
}

/// Re-publish until `done` observes the event (topology declaration / binding is
/// asynchronous on the consumer side).
///
/// A publish may transiently fail while the broker still considers the topology
/// incomplete (publisher confirms report `NO_ROUTE`); that is a retryable
/// transport condition, so the loop keeps trying instead of failing the test on
/// the first error.
async fn republish_until(
    publisher: &dyn IEventPublisher,
    message: &EventMessage,
    broadcast: bool,
    done: impl Fn() -> bool,
) -> (bool, Option<String>) {
    let mut last_error = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !done() && tokio::time::Instant::now() < deadline {
        let result = if broadcast {
            publisher.broadcast(message.clone()).await
        } else {
            publisher.publish(message.clone()).await
        };
        if let Err(error) = result {
            last_error = Some(error.to_string());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    (done(), last_error)
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
    // depend on the consumer having finished its queue declaration/bind. A
    // transient `NO_ROUTE` while the topology settles is retried.
    let message = EventMessage::from_event(&TestEvent { value: 1 }).expect("encode event");
    let mut publish_error = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while counted.load(Ordering::SeqCst) == 0 && tokio::time::Instant::now() < deadline {
        if let Err(error) = publisher.publish(message.clone()).await {
            publish_error = Some(error.to_string());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

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

    assert!(
        counted.load(Ordering::SeqCst) > 0,
        "the event handler did not receive the published event (last publish error: {publish_error:?})"
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

#[tokio::test]
async fn broadcast_roundtrip() {
    let Ok(url) = std::env::var("WEBX_RABBITMQ_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let counted = Arc::new(AtomicU32::new(0));
    let provider = provider_with_counting_handler(Arc::clone(&counted));
    let dispatcher = Arc::new(EventDispatcher::new(provider));

    let connection = Arc::new(RabbitConnection::new(url.clone(), Duration::from_secs(5)));
    let options = RabbitMqOptions::new(url).with_topology(
        format!("webx.test.broadcast.exchange.{suffix}"),
        format!("webx.test.broadcast.routing.{suffix}"),
        format!("webx.test.broadcast.queue.{suffix}"),
    );
    let publisher = RabbitMqEventPublisher::new(Arc::clone(&connection), options.clone());
    let (ct, task) = spawn_consumer(&connection, options, Arc::clone(&dispatcher));

    let message = EventMessage::from_event(&TestEvent { value: 1 }).expect("encode event");
    let (delivered, publish_error) = republish_until(&publisher, &message, true, || {
        counted.load(Ordering::SeqCst) > 0
    })
    .await;

    stop_consumer(ct, task).await;

    assert!(
        delivered,
        "the handler did not receive the broadcast event (last publish error: {publish_error:?})"
    );
}

#[tokio::test]
async fn broadcast_reaches_every_node() {
    let Ok(url) = std::env::var("WEBX_RABBITMQ_URL") else {
        return;
    };
    let suffix = unique_suffix();
    let first_counted = Arc::new(AtomicU32::new(0));
    let second_counted = Arc::new(AtomicU32::new(0));

    let connection = Arc::new(RabbitConnection::new(url.clone(), Duration::from_secs(5)));
    // Two nodes share the deployment topology; each declares its own exclusive
    // auto-delete queue on the fanout exchange, so both receive a copy.
    let options = RabbitMqOptions::new(url).with_topology(
        format!("webx.test.broadcast-fanout.exchange.{suffix}"),
        format!("webx.test.broadcast-fanout.routing.{suffix}"),
        format!("webx.test.broadcast-fanout.queue.{suffix}"),
    );
    let publisher = RabbitMqEventPublisher::new(Arc::clone(&connection), options.clone());

    let (first_ct, first_task) = spawn_consumer(
        &connection,
        options.clone(),
        Arc::new(EventDispatcher::new(provider_with_counting_handler(
            Arc::clone(&first_counted),
        ))),
    );
    let (second_ct, second_task) = spawn_consumer(
        &connection,
        options,
        Arc::new(EventDispatcher::new(provider_with_counting_handler(
            Arc::clone(&second_counted),
        ))),
    );

    let message = EventMessage::from_event(&TestEvent { value: 1 }).expect("encode event");
    let (delivered, publish_error) = republish_until(&publisher, &message, true, || {
        first_counted.load(Ordering::SeqCst) > 0 && second_counted.load(Ordering::SeqCst) > 0
    })
    .await;

    stop_consumer(first_ct, first_task).await;
    stop_consumer(second_ct, second_task).await;

    assert!(
        delivered,
        "the broadcast nodes did not both receive the event (last publish error: {publish_error:?})"
    );
    assert!(
        first_counted.load(Ordering::SeqCst) > 0,
        "the first node did not receive the broadcast event"
    );
    assert!(
        second_counted.load(Ordering::SeqCst) > 0,
        "the second node did not receive the broadcast event"
    );
}
