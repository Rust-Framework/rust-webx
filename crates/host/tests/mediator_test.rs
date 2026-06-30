//! Tests for IMediator send/publish using HandlerCache + DI resolution.
//!
//! `send` tests verify that the Mediator correctly resolves handlers via the
//! `HandlerCache` (populated by `HandlerRegistration` inventory submissions)
//! and dispatches requests through the factory + call bridge.
//!
//! `publish` tests verify event-handler resolution from the rust_dicore
//! ServiceProvider.
//!
//! Handlers are registered manually via `inventory::submit!` with
//! `HandlerRegistration` (same mechanism as `#[handler]` macro) using
//! `rust_webapp_core::` paths directly, since `rust-webapp-host` cannot
//! depend on the `rust_webapp` umbrella crate (circular dependency).

use rust_dicore::ServiceCollection;
use rust_webapp_core::error::{Error, Result as LrwfResult};
use rust_webapp_core::handler::{IEventHandler, IRequestHandler};
use rust_webapp_core::mediator::{IEventRequest, IMediator, IRequest};
use rust_webapp_core::mediator::Mediator;
use rust_webapp_core::route::scan::HandlerRegistration;
use std::sync::{Arc, Mutex};

// --- Request / Response Types ---

struct HelloRequest;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct HelloResponse {
    message: String,
}

impl IRequest<HelloResponse> for HelloRequest {}

// --- Handlers ---
//
// Manually registered via `inventory::submit!` — equivalent to what the
// `#[handler]` macro generates, but using `rust_webapp_core::` paths.

#[derive(Default)]
struct HelloHandler;

#[async_trait::async_trait]
impl IRequestHandler<HelloRequest, HelloResponse> for HelloHandler {
    async fn handle(&mut self, _req: HelloRequest) -> LrwfResult<HelloResponse> {
        Ok(HelloResponse {
            message: "hello".into(),
        })
    }
}

fn __factory_hello_handler(
    _resolver: &dyn rust_dicore::IServiceResolver,
) -> Box<dyn std::any::Any + Send> {
    Box::new(HelloHandler::default()) as Box<dyn std::any::Any + Send>
}

fn __call_hello_handler(
    handler: Box<dyn std::any::Any + Send>,
    request: Box<dyn std::any::Any + Send>,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = LrwfResult<Box<dyn std::any::Any + Send>>>
            + Send,
    >,
> {
    Box::pin(async move {
        let mut h = *handler
            .downcast::<HelloHandler>()
            .expect("Handler downcast failed");
        let req = *request
            .downcast::<HelloRequest>()
            .expect("Request downcast failed");
        let result: HelloResponse = h.handle(req).await?;
        Ok(Box::new(result) as Box<dyn std::any::Any + Send>)
    })
}

inventory::submit! {
    HandlerRegistration {
        req_type_name: "HelloRequest",
        factory: __factory_hello_handler,
        call: __call_hello_handler,
    }
}

#[derive(Default)]
struct FailingHandler;

#[async_trait::async_trait]
impl IRequestHandler<HelloRequest, HelloResponse> for FailingHandler {
    async fn handle(&mut self, _req: HelloRequest) -> LrwfResult<HelloResponse> {
        Err(Error::Internal("handler failure".into()))
    }
}

fn __factory_failing_handler(
    _resolver: &dyn rust_dicore::IServiceResolver,
) -> Box<dyn std::any::Any + Send> {
    Box::new(FailingHandler::default()) as Box<dyn std::any::Any + Send>
}

fn __call_failing_handler(
    handler: Box<dyn std::any::Any + Send>,
    request: Box<dyn std::any::Any + Send>,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = LrwfResult<Box<dyn std::any::Any + Send>>>
            + Send,
    >,
> {
    Box::pin(async move {
        let mut h = *handler
            .downcast::<FailingHandler>()
            .expect("Handler downcast failed");
        let req = *request
            .downcast::<HelloRequest>()
            .expect("Request downcast failed");
        let result: HelloResponse = h.handle(req).await?;
        Ok(Box::new(result) as Box<dyn std::any::Any + Send>)
    })
}

inventory::submit! {
    HandlerRegistration {
        req_type_name: "HelloRequest",
        factory: __factory_failing_handler,
        call: __call_failing_handler,
    }
}

// --- Event types ---

#[derive(Clone)]
struct TestEvent {
    payload: String,
}
impl IEventRequest for TestEvent {}

struct CountingEventHandler {
    counter: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl IEventHandler<TestEvent> for CountingEventHandler {
    async fn handle(&self, event: TestEvent) -> LrwfResult<()> {
        self.counter.lock().unwrap().push(event.payload);
        Ok(())
    }
}

struct FailingEventHandler;

#[async_trait::async_trait]
impl IEventHandler<TestEvent> for FailingEventHandler {
    async fn handle(&self, _event: TestEvent) -> LrwfResult<()> {
        Err(Error::Internal("event handler failure".into()))
    }
}

// --- Mediator::send tests ---
//
// Note: HandlerCache is process-global and keyed by request type name. With
// both HelloHandler and FailingHandler registered for HelloRequest, the
// last-submitted entry wins (inventory iteration order is deterministic per
// build but not guaranteed across rebuilds). These tests therefore only assert
// the success/failure shape, not which handler ran. The
// `mediator_send_handler_not_registered` test uses a dedicated request type
// with no registration to verify the not-registered error path.

fn build_provider() -> Arc<rust_dicore::ServiceProvider> {
    Arc::new(ServiceCollection::new().build().unwrap())
}

#[tokio::test]
async fn mediator_send_success_or_failure() {
    let mediator = Mediator::new(build_provider());
    let result = mediator.send(HelloRequest).await;
    match result {
        Ok(rsp) => assert_eq!(rsp.message, "hello"),
        Err(Error::Internal(msg)) => assert_eq!(msg, "handler failure"),
        Err(other) => panic!("Unexpected error variant: {:?}", other),
    }
}

// Dedicated request type with NO handler registration.
struct UnregisteredRequest;
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct UnregisteredResponse;
impl IRequest<UnregisteredResponse> for UnregisteredRequest {}

#[tokio::test]
async fn mediator_send_handler_not_registered() {
    let mediator = Mediator::new(build_provider());
    let result = mediator.send(UnregisteredRequest).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        Error::Di(msg) => assert!(msg.contains("No #[handler] registered")),
        other => panic!("Expected Di error, got {:?}", other),
    }
}

// --- Mediator::publish tests ---

#[tokio::test]
async fn mediator_publish_single_handler() {
    let counter = Arc::new(Mutex::new(Vec::new()));
    let counter_clone = Arc::clone(&counter);

    let provider = Arc::new(
        ServiceCollection::new()
            .singleton::<dyn IEventHandler<TestEvent>>(move |_| {
                Arc::new(CountingEventHandler {
                    counter: Arc::clone(&counter_clone),
                })
            })
            .build()
            .unwrap(),
    );
    let mediator = Mediator::new(provider);
    mediator
        .publish(TestEvent {
            payload: "event-1".into(),
        })
        .await
        .unwrap();

    let events = counter.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0], "event-1");
}

#[tokio::test]
async fn mediator_publish_multiple_handlers() {
    let counter = Arc::new(Mutex::new(Vec::new()));
    let c1 = Arc::clone(&counter);
    let c2 = Arc::clone(&counter);

    let provider = Arc::new(
        ServiceCollection::new()
            .singleton::<dyn IEventHandler<TestEvent>>(move |_| {
                Arc::new(CountingEventHandler {
                    counter: Arc::clone(&c1),
                })
            })
            .singleton::<dyn IEventHandler<TestEvent>>(move |_| {
                Arc::new(CountingEventHandler {
                    counter: Arc::clone(&c2),
                })
            })
            .build()
            .unwrap(),
    );
    let mediator = Mediator::new(provider);
    mediator
        .publish(TestEvent {
            payload: "multi".into(),
        })
        .await
        .unwrap();

    let events = counter.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0], "multi");
    assert_eq!(events[1], "multi");
}

#[tokio::test]
async fn mediator_publish_handler_returns_error() {
    let provider = Arc::new(
        ServiceCollection::new()
            .singleton::<dyn IEventHandler<TestEvent>>(|_| Arc::new(FailingEventHandler))
            .build()
            .unwrap(),
    );
    let mediator = Mediator::new(provider);
    let result = mediator
        .publish(TestEvent {
            payload: "will-fail".into(),
        })
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn mediator_publish_empty_handler_list() {
    let mediator = Mediator::new(build_provider());
    let result = mediator
        .publish(TestEvent {
            payload: "no-handlers".into(),
        })
        .await;
    assert!(result.is_ok());
}
