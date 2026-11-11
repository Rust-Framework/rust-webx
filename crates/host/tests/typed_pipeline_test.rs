//! Typed pipeline behaviors (0.10) through the real `Mediator` path.
//!
//! Handlers are registered manually via `inventory::submit!` (the host crate
//! cannot depend on the `webx` umbrella), then dispatched inside a
//! `DispatchRuntime` so `dispatch_handler_cache()` resolves the registry.

// The legacy erased behavior is deliberately exercised for ordering.
#![allow(deprecated)]
#![allow(clippy::type_complexity)]

use std::any::Any;
use std::sync::{Arc, Mutex};

use rust_dix::ServiceCollection;
use webx_core::dispatch_runtime::DispatchRuntime;
use webx_core::error::{Error, Result};
use webx_core::handler::IRequestHandler;
use webx_core::mediator::{IMediator, IRequest, Mediator};
use webx_core::pipeline::{IErasedPipelineBehavior, IPipelineBehavior, Next};
use webx_core::route::ext::IServiceCollectionExt;
use webx_core::route::scan::{HandlerCache, HandlerRegistration};

// --- request / response ---

#[derive(Debug, Clone)]
struct PingRequest {
    n: u32,
}

#[derive(Debug, PartialEq)]
struct PingResponse {
    n: u32,
}

impl IRequest<PingResponse> for PingRequest {}

type EventLog = Arc<Mutex<Vec<String>>>;

fn log_of(resolver: &dyn rust_dix::IServiceResolver) -> EventLog {
    // `instance(Arc<Mutex<Vec<String>>>)` registers the inner `Mutex<...>` type;
    // `get_any` keys on that type and hands back `Arc<Mutex<...>>`.
    resolver
        .get_any(std::any::type_name::<Mutex<Vec<String>>>())
        .and_then(|service| service.downcast::<EventLog>().ok())
        .map(|service| Arc::clone(&*service))
        .unwrap_or_else(|| Arc::new(Mutex::new(Vec::new())))
}

// --- handler ---

struct PingHandler {
    log: EventLog,
}

#[async_trait::async_trait]
impl IRequestHandler<PingRequest, PingResponse> for PingHandler {
    async fn handle(&mut self, req: PingRequest) -> Result<PingResponse> {
        self.log.lock().unwrap().push("handler".to_string());
        Ok(PingResponse { n: req.n * 10 })
    }
}

fn factory(resolver: &dyn rust_dix::IServiceResolver) -> Result<Box<dyn Any + Send>> {
    Ok(Box::new(PingHandler {
        log: log_of(resolver),
    }) as Box<dyn Any + Send>)
}

fn call(
    handler: Box<dyn Any + Send>,
    request: Box<dyn Any + Send>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Box<dyn Any + Send>>> + Send>> {
    Box::pin(async move {
        let mut handler = *handler
            .downcast::<PingHandler>()
            .map_err(|_| Error::Internal("handler downcast failed".into()))?;
        let req = *request
            .downcast::<PingRequest>()
            .map_err(|_| Error::Internal("request downcast failed".into()))?;
        let response = handler.handle(req).await?;
        Ok(Box::new(response) as Box<dyn Any + Send>)
    })
}

inventory::submit! {
    HandlerRegistration {
        req_type_id: std::any::TypeId::of::<PingRequest>(),
        req_type_name: "TypedPipelinePingRequest",
        factory,
        call,
    }
}

// --- typed behaviors ---

#[derive(Default)]
struct AddOne;

#[async_trait::async_trait]
impl IPipelineBehavior<PingRequest, PingResponse> for AddOne {
    async fn handle(
        &self,
        req: PingRequest,
        next: Next<PingRequest, PingResponse>,
    ) -> Result<PingResponse> {
        next.run(PingRequest { n: req.n + 1 }).await
    }
}

#[derive(Default)]
struct ShortCircuit;

#[async_trait::async_trait]
impl IPipelineBehavior<PingRequest, PingResponse> for ShortCircuit {
    async fn handle(
        &self,
        _req: PingRequest,
        _next: Next<PingRequest, PingResponse>,
    ) -> Result<PingResponse> {
        Ok(PingResponse { n: 999 })
    }
}

struct LogBehavior {
    log: EventLog,
    tag: &'static str,
}

#[async_trait::async_trait]
impl IPipelineBehavior<PingRequest, PingResponse> for LogBehavior {
    async fn handle(
        &self,
        req: PingRequest,
        next: Next<PingRequest, PingResponse>,
    ) -> Result<PingResponse> {
        self.log
            .lock()
            .unwrap()
            .push(format!("{}-before", self.tag));
        let response = next.run(req).await?;
        self.log.lock().unwrap().push(format!("{}-after", self.tag));
        Ok(response)
    }
}

/// Legacy erased counterpart, used to prove the outer/inner ordering.
struct ErasedLogBehavior {
    log: EventLog,
}

#[allow(deprecated)]
#[async_trait::async_trait]
impl IErasedPipelineBehavior for ErasedLogBehavior {
    async fn handle(
        &self,
        req: Box<dyn Any + Send>,
        next: webx_core::pipeline::BoxedNextFn,
    ) -> Result<Box<dyn Any + Send>> {
        self.log.lock().unwrap().push("erased-before".to_string());
        let response = next(req).await?;
        self.log.lock().unwrap().push("erased-after".to_string());
        Ok(response)
    }
}

// --- test harness ---

async fn send(provider: &Arc<rust_dix::ServiceProvider>, req: PingRequest) -> Result<PingResponse> {
    let runtime = Arc::new(DispatchRuntime::new(
        Arc::clone(provider),
        Arc::new(HandlerCache::build()),
    ));
    runtime
        .run(async {
            let mediator = Mediator::new(Arc::clone(provider));
            mediator.send(req).await
        })
        .await
}

fn provider_with<F>(configure: F) -> (Arc<rust_dix::ServiceProvider>, EventLog)
where
    F: FnOnce(ServiceCollection) -> ServiceCollection,
{
    let log: EventLog = Arc::new(Mutex::new(Vec::new()));
    let provider = configure(ServiceCollection::new().instance(Arc::clone(&log)))
        .build()
        .expect("provider must build");
    (provider, log)
}

#[tokio::test]
async fn typed_behavior_rewrites_the_request_for_the_handler() {
    let (provider, log) =
        provider_with(|svc| svc.add_typed_pipeline::<AddOne, PingRequest, PingResponse>());

    let response = send(&provider, PingRequest { n: 4 })
        .await
        .expect("send must succeed");
    assert_eq!(
        response,
        PingResponse { n: 50 },
        "(4+1)*10, so the handler saw the rewritten request"
    );
    assert_eq!(log.lock().unwrap().as_slice(), ["handler"]);
}

#[tokio::test]
async fn typed_behavior_short_circuit_returns_the_response_without_the_handler() {
    let (provider, log) =
        provider_with(|svc| svc.add_typed_pipeline::<ShortCircuit, PingRequest, PingResponse>());

    let response = send(&provider, PingRequest { n: 1 })
        .await
        .expect("send must succeed");
    assert_eq!(
        response,
        PingResponse { n: 999 },
        "the typed short-circuit value is returned as-is"
    );
    assert!(
        log.lock().unwrap().is_empty(),
        "the handler must not run when a typed behavior short-circuits"
    );
}

#[tokio::test]
async fn erased_behaviors_run_outside_typed_behaviors() {
    let log: EventLog = Arc::new(Mutex::new(Vec::new()));
    let provider = ServiceCollection::new()
        .instance(Arc::clone(&log))
        .add_pipeline_with({
            let log = Arc::clone(&log);
            move |_| {
                Arc::new(ErasedLogBehavior {
                    log: Arc::clone(&log),
                }) as Arc<dyn IErasedPipelineBehavior>
            }
        })
        .add_typed_pipeline_with({
            let log = Arc::clone(&log);
            move |_| {
                Arc::new(LogBehavior {
                    log: Arc::clone(&log),
                    tag: "typed",
                }) as Arc<dyn IPipelineBehavior<PingRequest, PingResponse>>
            }
        })
        .build()
        .expect("provider must build");

    let response = send(&provider, PingRequest { n: 2 })
        .await
        .expect("send must succeed");
    assert_eq!(response, PingResponse { n: 20 });
    assert_eq!(
        log.lock().unwrap().as_slice(),
        [
            "erased-before",
            "typed-before",
            "handler",
            "typed-after",
            "erased-after"
        ],
        "legacy erased behaviors are the outer chain, typed behaviors the inner chain"
    );
}
