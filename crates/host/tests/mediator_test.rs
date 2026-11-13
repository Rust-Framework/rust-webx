//! Tests for IMediator send/publish using HandlerCache + DI resolution.
//!
//! `send` tests verify that the Mediator correctly resolves handlers via the
//! `HandlerCache` (populated by `HandlerRegistration` inventory submissions)
//! and dispatches requests through the factory + call bridge.
//!
//! `publish` tests verify event-handler resolution from the rust_dix
//! ServiceProvider.
//!
//! Handlers are registered manually via `inventory::submit!` with
//! `HandlerRegistration` (same mechanism as `#[handler]` macro) using
//! `webx_core::` paths directly, since `rust-webx-host` cannot
//! depend on the `webx` umbrella crate (circular dependency).

// Manual handler-bridge functions mimic `#[handler]` macro output, whose
// `Pin<Box<dyn Future<Output = Result<Box<dyn Any + Send>>> + Send>>` return
// type is inherent to the dispatch bridge pattern.
#![allow(clippy::type_complexity)]

use rust_dix::ServiceCollection;
use std::sync::{Arc, Mutex};
use webx_core::error::{ActionResult, Error, Result as LrwfResult};
use webx_core::handler::{IEventHandler, IRequestHandler};
use webx_core::mediator::Mediator;
use webx_core::mediator::{IEventRequest, IMediator, IRequest};
use webx_core::route::scan::{HandlerCache, HandlerRegistration};

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
// `#[handler]` macro generates, but using `webx_core::` paths.

#[derive(Default)]
struct HelloHandler;

#[async_trait::async_trait]
impl IRequestHandler<HelloRequest, HelloResponse> for HelloHandler {
    async fn handle(&mut self, _req: HelloRequest) -> ActionResult<HelloResponse> {
        Ok(HelloResponse {
            message: "hello".into(),
        })
    }
}

fn __factory_hello_handler(
    _resolver: &dyn rust_dix::IServiceResolver,
) -> LrwfResult<Box<dyn std::any::Any + Send>> {
    Ok(Box::new(HelloHandler) as Box<dyn std::any::Any + Send>)
}

fn __call_hello_handler(
    handler: Box<dyn std::any::Any + Send>,
    request: Box<dyn std::any::Any + Send>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = LrwfResult<Box<dyn std::any::Any + Send>>> + Send>,
> {
    Box::pin(async move {
        let mut h = *handler
            .downcast::<HelloHandler>()
            .map_err(|_| Error::Internal("Handler downcast failed".into()))?;
        let req = *request
            .downcast::<HelloRequest>()
            .map_err(|_| Error::Internal("Request downcast failed".into()))?;
        let result: HelloResponse = h.handle(req).await?;
        Ok(Box::new(result) as Box<dyn std::any::Any + Send>)
    })
}

inventory::submit! {
    HandlerRegistration {
        req_type_id: std::any::TypeId::of::<HelloRequest>(),
        req_type_name: "HelloRequest",
        factory: __factory_hello_handler,
        call: __call_hello_handler,
    }
}

#[derive(Default)]
struct FailingHelloRequest;

impl IRequest<HelloResponse> for FailingHelloRequest {}

#[derive(Default)]
struct FailingHandler;

#[async_trait::async_trait]
impl IRequestHandler<FailingHelloRequest, HelloResponse> for FailingHandler {
    async fn handle(&mut self, _req: FailingHelloRequest) -> ActionResult<HelloResponse> {
        Err(Error::Internal("handler failure".into()))
    }
}

fn __factory_failing_handler(
    _resolver: &dyn rust_dix::IServiceResolver,
) -> LrwfResult<Box<dyn std::any::Any + Send>> {
    Ok(Box::new(FailingHandler) as Box<dyn std::any::Any + Send>)
}

fn __call_failing_handler(
    handler: Box<dyn std::any::Any + Send>,
    request: Box<dyn std::any::Any + Send>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = LrwfResult<Box<dyn std::any::Any + Send>>> + Send>,
> {
    Box::pin(async move {
        let mut h = *handler
            .downcast::<FailingHandler>()
            .map_err(|_| Error::Internal("Handler downcast failed".into()))?;
        let req = *request
            .downcast::<FailingHelloRequest>()
            .map_err(|_| Error::Internal("Request downcast failed".into()))?;
        let result: HelloResponse = h.handle(req).await?;
        Ok(Box::new(result) as Box<dyn std::any::Any + Send>)
    })
}

inventory::submit! {
    HandlerRegistration {
        req_type_id: std::any::TypeId::of::<FailingHelloRequest>(),
        req_type_name: "FailingHelloRequest",
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

fn build_provider() -> Arc<rust_dix::ServiceProvider> {
    ServiceCollection::new().build().unwrap()
}

/// `Mediator` driven in-process over the compiled handler registry.
///
/// 1.0 removed the task-local dispatch runtime: the provider and registry are
/// explicit handles, exactly as `Host::build` wires them.
struct TestMediator {
    mediator: Mediator,
}

impl TestMediator {
    fn new(provider: Arc<rust_dix::ServiceProvider>) -> Self {
        Self {
            mediator: Mediator::new(provider, Arc::new(HandlerCache::build())),
        }
    }

    async fn send<T, R>(&self, req: T) -> LrwfResult<R>
    where
        T: IRequest<R> + Send + 'static,
        R: Send + 'static,
    {
        self.mediator.send(req).await
    }

    async fn publish<T: IEventRequest>(&self, event: T) -> LrwfResult<()> {
        self.mediator.publish(event).await
    }
}

#[tokio::test]
async fn mediator_send_success() {
    let mediator = TestMediator::new(build_provider());
    let result = mediator.send(HelloRequest).await;
    let rsp = result.expect("send should succeed");
    assert_eq!(rsp.message, "hello");
}

#[tokio::test]
async fn mediator_send_handler_failure() {
    let mediator = TestMediator::new(build_provider());
    let result = mediator.send(FailingHelloRequest).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        Error::Internal(msg) => assert_eq!(msg, "handler failure"),
        other => panic!("Expected Internal error, got {:?}", other),
    }
}

// Dedicated request type with NO handler registration.
struct UnregisteredRequest;
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct UnregisteredResponse;
impl IRequest<UnregisteredResponse> for UnregisteredRequest {}

#[tokio::test]
async fn mediator_send_handler_not_registered() {
    let mediator = TestMediator::new(build_provider());
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

    let provider = ServiceCollection::new()
        .singleton::<dyn IEventHandler<TestEvent>>(move |_| {
            Arc::new(CountingEventHandler {
                counter: Arc::clone(&counter_clone),
            })
        })
        .build()
        .unwrap();
    let mediator = TestMediator::new(provider);
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

    let provider = ServiceCollection::new()
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
        .unwrap();
    let mediator = TestMediator::new(provider);
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
    let provider = ServiceCollection::new()
        .singleton::<dyn IEventHandler<TestEvent>>(|_| Arc::new(FailingEventHandler))
        .build()
        .unwrap();
    let mediator = TestMediator::new(provider);
    let result = mediator
        .publish(TestEvent {
            payload: "will-fail".into(),
        })
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn mediator_publish_empty_handler_list() {
    let mediator = TestMediator::new(build_provider());
    let result = mediator
        .publish(TestEvent {
            payload: "no-handlers".into(),
        })
        .await;
    assert!(result.is_ok());
}

// --- Mediator::send scope provider test ---
//
// Verifies P0-5 fix: `Mediator::send` creates a per-call scope so that Scoped
// services resolve to a single shared instance within one send invocation
// (matching the HTTP dispatch path). Before the fix, send used the root
// provider, which made Scoped services degrade to transient (fresh instance
// per resolution, no within-call caching).

use std::sync::atomic::{AtomicU32, Ordering};

static SCOPED_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Scoped service that records the order in which it was constructed.
struct ScopedService {
    instance_id: u32,
}

struct ScopeProbeRequest;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ScopeProbeResponse {
    /// "same" if two resolutions within one send returned the same instance,
    /// "different" otherwise.
    within_call: String,
    /// The instance id observed on the first resolution.
    first_id: u32,
}

impl IRequest<ScopeProbeResponse> for ScopeProbeRequest {}

/// Factory that resolves `ScopedService` twice from the same resolver and
/// records whether the two resolutions returned the same instance.
fn __factory_scope_probe_handler(
    resolver: &dyn rust_dix::IServiceResolver,
) -> LrwfResult<Box<dyn std::any::Any + Send>> {
    // Use get_any (the only non-Sized-bound resolver method) + downcast.
    let key = std::any::type_name::<ScopedService>();
    let a: Arc<ScopedService> = resolver
        .get_any(key)
        .and_then(|a| a.downcast::<Arc<ScopedService>>().ok())
        .map(|d| Arc::clone(&*d))
        .ok_or_else(|| Error::Internal("ScopedService not registered".into()))?;
    let b: Arc<ScopedService> = resolver
        .get_any(key)
        .and_then(|a| a.downcast::<Arc<ScopedService>>().ok())
        .map(|d| Arc::clone(&*d))
        .ok_or_else(|| Error::Internal("ScopedService not registered".into()))?;
    let within_call = if std::ptr::eq(Arc::as_ptr(&a), Arc::as_ptr(&b)) {
        "same"
    } else {
        "different"
    };
    let first_id = a.instance_id;
    Ok(Box::new(ScopeProbeResult {
        within_call: within_call.to_string(),
        first_id,
    }) as Box<dyn std::any::Any + Send>)
}

struct ScopeProbeResult {
    within_call: String,
    first_id: u32,
}

fn __call_scope_probe_handler(
    handler: Box<dyn std::any::Any + Send>,
    _request: Box<dyn std::any::Any + Send>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = LrwfResult<Box<dyn std::any::Any + Send>>> + Send>,
> {
    Box::pin(async move {
        let h = *handler
            .downcast::<ScopeProbeResult>()
            .map_err(|_| Error::Internal("ScopeProbeResult downcast failed".into()))?;
        let rsp = ScopeProbeResponse {
            within_call: h.within_call,
            first_id: h.first_id,
        };
        Ok(Box::new(rsp) as Box<dyn std::any::Any + Send>)
    })
}

inventory::submit! {
    HandlerRegistration {
        req_type_id: std::any::TypeId::of::<ScopeProbeRequest>(),
        req_type_name: "ScopeProbeRequest",
        factory: __factory_scope_probe_handler,
        call: __call_scope_probe_handler,
    }
}

#[tokio::test]
async fn mediator_send_uses_per_call_scope_for_scoped_services() {
    SCOPED_COUNTER.store(0, Ordering::SeqCst);

    let provider = ServiceCollection::new()
        .scoped::<ScopedService>(|_| {
            let id = SCOPED_COUNTER.fetch_add(1, Ordering::SeqCst) + 1;
            Arc::new(ScopedService { instance_id: id })
        })
        .build()
        .unwrap();
    let mediator = TestMediator::new(provider);

    let r1 = mediator
        .send(ScopeProbeRequest)
        .await
        .expect("send #1 failed");
    let r2 = mediator
        .send(ScopeProbeRequest)
        .await
        .expect("send #2 failed");

    // Within a single send, both resolutions must return the same instance
    // (Scoped caching within the per-call scope).
    assert_eq!(
        r1.within_call, "same",
        "first send: scope not caching scoped service"
    );
    assert_eq!(
        r2.within_call, "same",
        "second send: scope not caching scoped service"
    );

    // Across sends, a new scope means a fresh scoped instance.
    assert_eq!(r1.first_id, 1, "first send should observe instance #1");
    assert_eq!(r2.first_id, 2, "second send should observe instance #2");
}
