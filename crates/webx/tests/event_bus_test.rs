//! Event dispatch isolation and transport replacement.
//!
//! `IMediator::publish` must return after the transport accepted the event 閳?//! never after (or because of) a handler. The default transport is the bounded
//! in-memory queue; an application can replace it with a broker adapter that
//! implements `IEventPublisher` and consumes through `EventDispatcher`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use webx::*;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct OrderPlaced {
    id: u32,
}
impl IEventRequest for OrderPlaced {}

#[derive(Default)]
struct SlowState {
    started: tokio::sync::Notify,
    finished: AtomicBool,
}

#[derive(Inject)]
struct SlowHandler {
    #[inject]
    state: Arc<SlowState>,
}

#[handler(inject)]
#[async_trait::async_trait]
impl IEventHandler<OrderPlaced> for SlowHandler {
    async fn handle(&self, _event: OrderPlaced) -> Result<()> {
        self.state.started.notify_waiters();
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.state.finished.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
struct FailingState {
    failed: AtomicBool,
}

#[derive(Inject)]
struct FailingHandler {
    #[inject]
    state: Arc<FailingState>,
}

#[handler(inject)]
#[async_trait::async_trait]
impl IEventHandler<OrderPlaced> for FailingHandler {
    async fn handle(&self, _event: OrderPlaced) -> Result<()> {
        self.state.failed.store(true, Ordering::SeqCst);
        Err(Error::internal("subscriber failed"))
    }
}

async fn start_hosted_services(host: &Host) -> Vec<Arc<dyn IHostedService>> {
    let services = host.provider().get_all::<dyn IHostedService>();
    for service in &services {
        service.start().await.expect("hosted service must start");
    }
    services
}

async fn stop_hosted_services(services: &[Arc<dyn IHostedService>]) {
    for service in services.iter().rev() {
        let _ = service.stop().await;
    }
}

#[tokio::test]
async fn publish_returns_before_handlers_and_isolates_their_failure() {
    let slow_state = Arc::new(SlowState::default());
    let failing_state = Arc::new(FailingState::default());

    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register({
            let slow_state = Arc::clone(&slow_state);
            let failing_state = Arc::clone(&failing_state);
            move |svc| {
                svc.instance::<SlowState>(slow_state)
                    .instance::<FailingState>(failing_state)
                    .add_mediator()
            }
        })
        .try_build()
        .expect("host must build");

    let services = start_hosted_services(&host).await;
    let mediator = host.provider().get::<Mediator>().expect("mediator");

    let published_at = Instant::now();
    mediator
        .publish(OrderPlaced { id: 1 })
        .await
        .expect("the event must be accepted");
    assert!(
        published_at.elapsed() < Duration::from_millis(100),
        "publish waited for a handler: {:?}",
        published_at.elapsed()
    );
    assert!(
        !slow_state.finished.load(Ordering::SeqCst),
        "publish ran the handler inline"
    );
    assert!(
        !failing_state.failed.load(Ordering::SeqCst),
        "publish ran the handler inline"
    );

    tokio::time::timeout(Duration::from_secs(5), async {
        while !(slow_state.finished.load(Ordering::SeqCst)
            && failing_state.failed.load(Ordering::SeqCst))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the worker must eventually run both handlers");

    // The failing handler is reported to the transport (logged), not to the
    // publisher 閳?which already received `Ok`.
    assert!(
        failing_state.failed.load(Ordering::SeqCst),
        "the failing handler must still run"
    );
    assert!(
        slow_state.finished.load(Ordering::SeqCst),
        "the remaining handler must still run"
    );

    stop_hosted_services(&services).await;
}

#[tokio::test]
async fn publish_without_local_registration_fails_closed() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(|svc| svc.add_mediator())
        .try_build()
        .expect("host must build");
    let mediator = host.provider().get::<Mediator>().expect("mediator");

    let error = mediator
        .publish(UnhandledEvent { value: 1 })
        .await
        .expect_err("an event with no local handler must not be dropped silently");
    assert!(
        error
            .operator()
            .is_some_and(|message| message.contains("event registration")),
        "expected a DI error, got {error:?}"
    );
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct UnhandledEvent {
    value: u32,
}
impl IEventRequest for UnhandledEvent {}

struct RecordingPublisher {
    messages: Mutex<Vec<EventMessage>>,
}

#[async_trait::async_trait]
impl IEventPublisher for RecordingPublisher {
    async fn publish(&self, message: EventMessage) -> Result<()> {
        self.messages.lock().unwrap().push(message);
        Ok(())
    }
}

#[tokio::test]
async fn a_broker_adapter_replaces_the_default_transport() {
    let publisher = Arc::new(RecordingPublisher {
        messages: Mutex::new(Vec::new()),
    });
    let publisher_for_host: Arc<dyn IEventPublisher> =
        Arc::clone(&publisher) as Arc<dyn IEventPublisher>;
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_event_publisher(publisher_for_host)
        .register(|svc| svc.add_mediator())
        .try_build()
        .expect("host must build");
    let mediator = host.provider().get::<Mediator>().expect("mediator");

    mediator
        .publish(OrderPlaced { id: 7 })
        .await
        .expect("the adapter must accept the event");

    let messages = publisher.messages.lock().unwrap();
    assert_eq!(messages.len(), 1, "the adapter must receive the event");
    assert_eq!(
        messages[0].event_name,
        <OrderPlaced as IEventRequest>::event_name()
    );
    assert_eq!(
        messages[0].decode::<OrderPlaced>().unwrap(),
        OrderPlaced { id: 7 }
    );
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct InvoiceIssued {
    invoice_id: u32,
}
impl IEventRequest for InvoiceIssued {}

#[derive(Default)]
struct InvoiceState {
    seen: AtomicU32,
}

#[derive(Inject)]
struct InvoiceHandler {
    #[inject]
    state: Arc<InvoiceState>,
}

#[handler(inject)]
#[async_trait::async_trait]
impl IEventHandler<InvoiceIssued> for InvoiceHandler {
    async fn handle(&self, event: InvoiceIssued) -> Result<()> {
        self.state
            .seen
            .fetch_add(event.invoice_id, Ordering::SeqCst);
        Ok(())
    }
}

static DEFAULT_EVENT_COUNT: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct DefaultEvent {
    value: u32,
}
impl IEventRequest for DefaultEvent {}

#[derive(Default)]
struct DefaultHandler;

#[handler]
#[async_trait::async_trait]
impl IEventHandler<DefaultEvent> for DefaultHandler {
    async fn handle(&self, event: DefaultEvent) -> Result<()> {
        DEFAULT_EVENT_COUNT.fetch_add(event.value, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn default_event_handler_runs_without_di_registration() {
    DEFAULT_EVENT_COUNT.store(0, Ordering::SeqCst);

    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(|svc| svc.add_mediator())
        .try_build()
        .expect("host must build");
    let services = start_hosted_services(&host).await;
    let mediator = host.provider().get::<Mediator>().expect("mediator");
    mediator
        .publish(DefaultEvent { value: 5 })
        .await
        .expect("publish");
    tokio::time::timeout(Duration::from_secs(5), async {
        while DEFAULT_EVENT_COUNT.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("default handler must receive the event");
    assert_eq!(DEFAULT_EVENT_COUNT.load(Ordering::SeqCst), 5);
    stop_hosted_services(&services).await;
}

static INJECTED_EVENT_COUNT: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct InjectedEvent {
    value: u32,
}
impl IEventRequest for InjectedEvent {}

#[derive(Default)]
struct InjectedDependency;

#[derive(Inject)]
struct InjectedHandler {
    #[inject]
    dependency: Arc<InjectedDependency>,
}

#[handler(inject)]
#[async_trait::async_trait]
impl IEventHandler<InjectedEvent> for InjectedHandler {
    async fn handle(&self, event: InjectedEvent) -> Result<()> {
        let _ = &self.dependency;
        INJECTED_EVENT_COUNT.fetch_add(event.value, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn injected_event_handler_is_constructed_from_di_without_explicit_registration() {
    INJECTED_EVENT_COUNT.store(0, Ordering::SeqCst);
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(|svc| {
            svc.singleton::<InjectedDependency>(|_| Arc::new(InjectedDependency))
                .add_mediator()
        })
        .try_build()
        .expect("host must build");
    let services = start_hosted_services(&host).await;
    let mediator = host.provider().get::<Mediator>().expect("mediator");
    mediator
        .publish(InjectedEvent { value: 9 })
        .await
        .expect("publish");
    tokio::time::timeout(Duration::from_secs(5), async {
        while INJECTED_EVENT_COUNT.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("injected handler must receive the event");
    assert_eq!(INJECTED_EVENT_COUNT.load(Ordering::SeqCst), 9);
    stop_hosted_services(&services).await;
}

static FANOUT_A: AtomicU32 = AtomicU32::new(0);
static FANOUT_B: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct FanoutEvent {
    value: u32,
}
impl IEventRequest for FanoutEvent {}

#[derive(Default)]
struct FanoutA;

#[handler]
#[async_trait::async_trait]
impl IEventHandler<FanoutEvent> for FanoutA {
    async fn handle(&self, event: FanoutEvent) -> Result<()> {
        FANOUT_A.fetch_add(event.value, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
struct FanoutB;

#[handler]
#[async_trait::async_trait]
impl IEventHandler<FanoutEvent> for FanoutB {
    async fn handle(&self, event: FanoutEvent) -> Result<()> {
        FANOUT_B.fetch_add(event.value, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn multiple_handlers_for_one_event_all_run() {
    FANOUT_A.store(0, Ordering::SeqCst);
    FANOUT_B.store(0, Ordering::SeqCst);
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(|svc| svc.add_mediator())
        .try_build()
        .expect("host must build");
    let services = start_hosted_services(&host).await;
    let mediator = host.provider().get::<Mediator>().expect("mediator");
    mediator
        .publish(FanoutEvent { value: 3 })
        .await
        .expect("publish");
    tokio::time::timeout(Duration::from_secs(5), async {
        while FANOUT_A.load(Ordering::SeqCst) == 0 || FANOUT_B.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("both handlers must receive the event");
    assert_eq!(FANOUT_A.load(Ordering::SeqCst), 3);
    assert_eq!(FANOUT_B.load(Ordering::SeqCst), 3);
    stop_hosted_services(&services).await;
}

#[tokio::test]
async fn a_broker_consumer_routes_messages_through_the_event_dispatcher() {
    // Simulates the consumer half of a Redis/NATS adapter: it only has the
    // event name and the JSON payload.
    let state = Arc::new(InvoiceState::default());
    let provider = rust_dix::ServiceCollection::new()
        .instance::<InvoiceState>(Arc::clone(&state))
        .build()
        .expect("provider");

    let dispatcher = EventDispatcher::new(provider);
    let message = EventMessage::new(
        <InvoiceIssued as IEventRequest>::event_name(),
        serde_json::to_vec(&InvoiceIssued { invoice_id: 5 }).unwrap(),
    );
    dispatcher.dispatch(&message).await.expect("dispatch");

    assert_eq!(state.seen.load(Ordering::SeqCst), 5);
}
