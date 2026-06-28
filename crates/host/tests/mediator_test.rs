//! Tests for IMediator send/publish using DI resolution.
//!
//! These tests verify that the Mediator correctly resolves handlers
//! from the rust_dicore ServiceProvider and dispatches requests and events.

use rust_dicore::ServiceCollection;
use rust_webapp_core::error::{Error, Result as LrwfResult};
use rust_webapp_core::handler::{IEventHandler, IRequestHandler};
use rust_webapp_core::mediator::{IEventRequest, IMediator, IRequest};
use rust_webapp_core::mediator_impl::Mediator;
use std::sync::{Arc, Mutex};

// --- Request / Response Types ---

struct HelloRequest;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct HelloResponse {
    message: String,
}

impl IRequest<HelloResponse> for HelloRequest {}

// --- Handlers ---

#[derive(Default)]
struct HelloHandler;

#[async_trait::async_trait]
impl IRequestHandler<HelloRequest, HelloResponse> for HelloHandler {
    async fn handle(&self, _req: HelloRequest) -> LrwfResult<HelloResponse> {
        Ok(HelloResponse {
            message: "hello".into(),
        })
    }
}

#[derive(Default)]
struct FailingHandler;

#[async_trait::async_trait]
impl IRequestHandler<HelloRequest, HelloResponse> for FailingHandler {
    async fn handle(&self, _req: HelloRequest) -> LrwfResult<HelloResponse> {
        Err(Error::Internal("handler failure".into()))
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

#[tokio::test]
async fn mediator_send_success() {
    let provider = Arc::new(
        ServiceCollection::new()
            .singleton::<dyn IRequestHandler<HelloRequest, HelloResponse>>(|_| {
                Arc::new(HelloHandler::default())
            })
            .build()
            .unwrap(),
    );
    let mediator = Mediator::new(provider);
    let result = mediator.send(HelloRequest).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().message, "hello");
}

#[tokio::test]
async fn mediator_send_handler_not_registered() {
    let provider = Arc::new(ServiceCollection::new().build().unwrap());
    let mediator = Mediator::new(provider);
    let result = mediator.send(HelloRequest).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        Error::Di(msg) => assert!(msg.contains("No handler")),
        other => panic!("Expected Di error, got {:?}", other),
    }
}

#[tokio::test]
async fn mediator_send_handler_returns_error() {
    let provider = Arc::new(
        ServiceCollection::new()
            .singleton::<dyn IRequestHandler<HelloRequest, HelloResponse>>(|_| {
                Arc::new(FailingHandler::default())
            })
            .build()
            .unwrap(),
    );
    let mediator = Mediator::new(provider);
    let result = mediator.send(HelloRequest).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        Error::Internal(msg) => assert_eq!(msg, "handler failure"),
        other => panic!("Expected Internal error, got {:?}", other),
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
    let provider = Arc::new(ServiceCollection::new().build().unwrap());
    let mediator = Mediator::new(provider);
    let result = mediator
        .publish(TestEvent {
            payload: "no-handlers".into(),
        })
        .await;
    assert!(result.is_ok());
}
