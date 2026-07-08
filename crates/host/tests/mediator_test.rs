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
//! `rust_webx_core::` paths directly, since `rust-webx-host` cannot
//! depend on the `rust_webx` umbrella crate (circular dependency).

use rust_dix::ServiceCollection;
use rust_webx_core::error::{Error, Result as LrwfResult};
use rust_webx_core::handler::{IEventHandler, IRequestHandler};
use rust_webx_core::mediator::{IEventRequest, IMediator, IRequest};
use rust_webx_core::mediator::Mediator;
use rust_webx_core::route::scan::HandlerRegistration;
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
// `#[handler]` macro generates, but using `rust_webx_core::` paths.

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
    _resolver: &dyn rust_dix::IServiceResolver,
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
        req_type_id: std::any::TypeId::of::<HelloRequest>(),
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
    _resolver: &dyn rust_dix::IServiceResolver,
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
        req_type_id: std::any::TypeId::of::<HelloRequest>(),
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

fn build_provider() -> Arc<rust_dix::ServiceProvider> {
    ServiceCollection::new().build().unwrap()
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

    let provider = ServiceCollection::new()
            .singleton::<dyn IEventHandler<TestEvent>>(move |_| {
                Arc::new(CountingEventHandler {
                    counter: Arc::clone(&counter_clone),
                })
            })
            .build()
            .unwrap();
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
    let provider = ServiceCollection::new()
            .singleton::<dyn IEventHandler<TestEvent>>(|_| Arc::new(FailingEventHandler))
            .build()
            .unwrap();
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
) -> Box<dyn std::any::Any + Send> {
    // Use get_any (the only non-Sized-bound resolver method) + downcast.
    let key = std::any::type_name::<ScopedService>();
    let a: Arc<ScopedService> = resolver
        .get_any(key)
        .and_then(|a| a.downcast::<Arc<ScopedService>>().ok())
        .map(|d| Arc::clone(&*d))
        .expect("ScopedService not registered");
    let b: Arc<ScopedService> = resolver
        .get_any(key)
        .and_then(|a| a.downcast::<Arc<ScopedService>>().ok())
        .map(|d| Arc::clone(&*d))
        .expect("ScopedService not registered");
    let within_call = if std::ptr::eq(Arc::as_ptr(&a), Arc::as_ptr(&b)) {
        "same"
    } else {
        "different"
    };
    let first_id = a.instance_id;
    Box::new(ScopeProbeResult {
        within_call: within_call.to_string(),
        first_id,
    }) as Box<dyn std::any::Any + Send>
}

struct ScopeProbeResult {
    within_call: String,
    first_id: u32,
}

fn __call_scope_probe_handler(
    handler: Box<dyn std::any::Any + Send>,
    _request: Box<dyn std::any::Any + Send>,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = LrwfResult<Box<dyn std::any::Any + Send>>>
            + Send,
    >,
> {
    Box::pin(async move {
        let h = *handler
            .downcast::<ScopeProbeResult>()
            .expect("ScopeProbeResult downcast failed");
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
    let mediator = Mediator::new(provider);

    let r1 = mediator.send(ScopeProbeRequest).await.expect("send #1 failed");
    let r2 = mediator.send(ScopeProbeRequest).await.expect("send #2 failed");

    // Within a single send, both resolutions must return the same instance
    // (Scoped caching within the per-call scope).
    assert_eq!(r1.within_call, "same", "first send: scope not caching scoped service");
    assert_eq!(r2.within_call, "same", "second send: scope not caching scoped service");

    // Across sends, a new scope means a fresh scoped instance.
    assert_eq!(r1.first_id, 1, "first send should observe instance #1");
    assert_eq!(r2.first_id, 2, "second send should observe instance #2");
}

// --- Pipeline behavior chain tests ---
//
// Verifies P0-3: IPipelineBehavior chain construction and execution.
// Behaviors wrap the handler in a MediatR-style chain: each behavior can
// inspect/modify the request, short-circuit, or pass through to the next.

use rust_webx_core::pipeline::{BoxedNextFn, IPipelineBehavior};

struct BehaviorProbeRequest;

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
struct BehaviorProbeResponse {
    message: String,
    source: String,
}

impl IRequest<BehaviorProbeResponse> for BehaviorProbeRequest {}

#[derive(Default)]
struct BehaviorProbeHandler;

#[async_trait::async_trait]
impl IRequestHandler<BehaviorProbeRequest, BehaviorProbeResponse> for BehaviorProbeHandler {
    async fn handle(&mut self, _req: BehaviorProbeRequest) -> LrwfResult<BehaviorProbeResponse> {
        Ok(BehaviorProbeResponse {
            message: "from-handler".into(),
            source: "handler".into(),
        })
    }
}

fn __factory_behavior_probe_handler(
    _resolver: &dyn rust_dix::IServiceResolver,
) -> Box<dyn std::any::Any + Send> {
    Box::new(BehaviorProbeHandler::default()) as Box<dyn std::any::Any + Send>
}

fn __call_behavior_probe_handler(
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
            .downcast::<BehaviorProbeHandler>()
            .expect("Handler downcast failed");
        let req = *request
            .downcast::<BehaviorProbeRequest>()
            .expect("Request downcast failed");
        let result: BehaviorProbeResponse = h.handle(req).await?;
        Ok(Box::new(result) as Box<dyn std::any::Any + Send>)
    })
}

inventory::submit! {
    HandlerRegistration {
        req_type_id: std::any::TypeId::of::<BehaviorProbeRequest>(),
        req_type_name: "BehaviorProbeRequest",
        factory: __factory_behavior_probe_handler,
        call: __call_behavior_probe_handler,
    }
}

/// Behavior that records the order it was called relative to the handler.
struct TrackingBehavior {
    called: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl IPipelineBehavior for TrackingBehavior {
    async fn handle(
        &self,
        req: Box<dyn std::any::Any + Send>,
        next: BoxedNextFn,
    ) -> LrwfResult<Box<dyn std::any::Any + Send>> {
        self.called.fetch_add(1, Ordering::SeqCst);
        next(req).await
    }
}

/// Behavior that short-circuits the chain without calling next.
struct ShortCircuitBehavior;

#[async_trait::async_trait]
impl IPipelineBehavior for ShortCircuitBehavior {
    async fn handle(
        &self,
        _req: Box<dyn std::any::Any + Send>,
        _next: BoxedNextFn,
    ) -> LrwfResult<Box<dyn std::any::Any + Send>> {
        Ok(Box::new(BehaviorProbeResponse {
            message: "short-circuited".into(),
            source: "behavior".into(),
        }))
    }
}

#[tokio::test]
async fn mediator_send_pipeline_behavior_executes_before_handler() {
    let behavior_called = Arc::new(AtomicU32::new(0));
    let behavior = Arc::new(TrackingBehavior {
        called: Arc::clone(&behavior_called),
    });

    let provider = ServiceCollection::new()
            .singleton::<dyn IPipelineBehavior>(move |_| {
                Arc::clone(&behavior) as Arc<dyn IPipelineBehavior>
            })
            .build()
            .unwrap();
    let mediator = Mediator::new(provider);

    let rsp = mediator
        .send(BehaviorProbeRequest)
        .await
        .expect("send failed");

    assert_eq!(behavior_called.load(Ordering::SeqCst), 1, "behavior should be called once");
    assert_eq!(rsp.source, "handler", "response should come from handler");
    assert_eq!(rsp.message, "from-handler");
}

#[tokio::test]
async fn mediator_send_pipeline_behavior_can_short_circuit() {
    let provider = ServiceCollection::new()
            .singleton::<dyn IPipelineBehavior>(|_| {
                Arc::new(ShortCircuitBehavior) as Arc<dyn IPipelineBehavior>
            })
            .build()
            .unwrap();
    let mediator = Mediator::new(provider);

    let rsp = mediator
        .send(BehaviorProbeRequest)
        .await
        .expect("send failed");

    assert_eq!(rsp.source, "behavior", "response should come from behavior (short-circuit)");
    assert_eq!(rsp.message, "short-circuited");
}

#[tokio::test]
async fn mediator_send_pipeline_empty_chain_works() {
    // No behaviors registered — chain is just the terminal handler.
    let provider = build_provider();
    let mediator = Mediator::new(provider);

    let rsp = mediator
        .send(BehaviorProbeRequest)
        .await
        .expect("send failed");

    assert_eq!(rsp.source, "handler");
    assert_eq!(rsp.message, "from-handler");
}
