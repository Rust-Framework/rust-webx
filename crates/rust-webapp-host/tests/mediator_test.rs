//! Tests for IMediator send/publish using the HandlerCache.
//!
//! These tests verify that the Mediator correctly resolves handlers
//! from the HandlerCache and dispatches requests and events.

use rust_dicore::ServiceCollection;
use rust_webapp_core::di::scan::{HandlerCache, ResponseData};
use rust_webapp_core::error::{Error, Result as LrwfResult};
use rust_webapp_core::handler::IEventHandler;
use rust_webapp_core::handler::IRequestHandler;
use rust_webapp_core::mediator::{IEventRequest, IMediator, IRequest};
use rust_webapp_core::mediator_impl::Mediator;
use std::any::Any;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

// â”€â”€â”€ Request / Response Types â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

struct HelloRequest;
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct HelloResponse {
    message: String,
}
impl IRequest<HelloResponse> for HelloRequest {}

// â”€â”€â”€ Handler (native async fn â€?no #[async_trait]) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

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

// â”€â”€â”€ Type-erased call bridge helpers â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[allow(clippy::type_complexity)]
fn make_call_bridge<T, R, H>() -> fn(
    handler: &Arc<dyn Any + Send + Sync>,
    request: Box<dyn Any + Send>,
    claims: Option<Box<dyn rust_webapp_core::auth::IClaims>>,
) -> Pin<
    Box<dyn std::future::Future<Output = LrwfResult<ResponseData>> + Send>,
>
where
    T: IRequest<R> + Send + 'static,
    R: serde::Serialize + Send + 'static,
    H: IRequestHandler<T, R> + Default + 'static,
{
    |handler, request, _claims| {
        let handler = Arc::clone(handler);
        Box::pin(async move {
            let h = handler
                .downcast_ref::<Arc<H>>()
                .expect("Handler downcast failed");
            let req = *request.downcast::<T>().expect("Request downcast failed");
            let result = h.handle(req).await?;
            let json_bytes = serde_json::to_vec(&result).unwrap_or_default();
            Ok(ResponseData {
                status: 200,
                content_type: "application/json".to_string(),
                body: json_bytes,
            })
        })
    }
}

fn build_cache(req_type_name: &'static str, handler_type: &str) -> HandlerCache {
    let mut entries = HashMap::new();
    if handler_type == "hello" {
        #[allow(clippy::default_constructed_unit_structs)]
        let handler: Arc<dyn Any + Send + Sync> = Arc::new(Arc::new(HelloHandler::default()));
        entries.insert(
            req_type_name,
            Arc::new(rust_webapp_core::di::scan::HandlerEntry {
                handler,
                call: make_call_bridge::<HelloRequest, HelloResponse, HelloHandler>(),
            }),
        );
    } else if handler_type == "failing" {
        #[allow(clippy::default_constructed_unit_structs)]
        let handler: Arc<dyn Any + Send + Sync> = Arc::new(Arc::new(FailingHandler::default()));
        entries.insert(
            req_type_name,
            Arc::new(rust_webapp_core::di::scan::HandlerEntry {
                handler,
                call: make_call_bridge::<HelloRequest, HelloResponse, FailingHandler>(),
            }),
        );
    }
    HandlerCache { entries }
}

fn build_empty_cache() -> HandlerCache {
    HandlerCache {
        entries: HashMap::new(),
    }
}

// â”€â”€â”€ Event types â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

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

// â”€â”€â”€ Mediator::send tests â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
async fn mediator_send_success() {
    let cache = build_cache(std::any::type_name::<HelloRequest>(), "hello");
    let provider = Arc::new(ServiceCollection::new().build().unwrap());
    let mediator = Mediator::new(Arc::new(cache), provider);
    let result = mediator.send(HelloRequest).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().message, "hello");
}

#[tokio::test]
async fn mediator_send_handler_not_registered() {
    let cache = build_empty_cache();
    let provider = Arc::new(ServiceCollection::new().build().unwrap());
    let mediator = Mediator::new(Arc::new(cache), provider);
    let result = mediator.send(HelloRequest).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        Error::Di(msg) => assert!(msg.contains("No handler")),
        other => panic!("Expected Di error, got {:?}", other),
    }
}

#[tokio::test]
async fn mediator_send_handler_returns_error() {
    let cache = build_cache(std::any::type_name::<HelloRequest>(), "failing");
    let provider = Arc::new(ServiceCollection::new().build().unwrap());
    let mediator = Mediator::new(Arc::new(cache), provider);
    let result = mediator.send(HelloRequest).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        Error::Internal(msg) => assert_eq!(msg, "handler failure"),
        other => panic!("Expected Internal error, got {:?}", other),
    }
}

// â”€â”€â”€ Mediator::publish tests â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

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
    let cache = build_empty_cache();
    let mediator = Mediator::new(Arc::new(cache), provider);
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
    let cache = build_empty_cache();
    let mediator = Mediator::new(Arc::new(cache), provider);
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
    let cache = build_empty_cache();
    let mediator = Mediator::new(Arc::new(cache), provider);
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
    let cache = build_empty_cache();
    let mediator = Mediator::new(Arc::new(cache), provider);
    let result = mediator
        .publish(TestEvent {
            payload: "no-handlers".into(),
        })
        .await;
    assert!(result.is_ok());
}
