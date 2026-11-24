//! 2.4-4: unified middleware `Next` semantics end-to-end.
//!
//! A continuation-style middleware wraps the rest of the chain (timeout /
//! retry / circuit-breaker shape) while legacy `invoke`/`after` stages keep
//! working in the same pipeline. Errors raised downstream are converted to a
//! response *before* the reverse `after` pass, so post-processing middleware
//! observes the real status.

use std::net::TcpListener;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use webx::*;

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

async fn spawn_host(middlewares: Vec<Arc<dyn IMiddleware>>) -> (String, webx::ServerHandle) {
    let port = find_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut builder = Host::builder().mode(AppMode::Development).no_spa();
    for middleware in middlewares {
        let middleware = Arc::clone(&middleware);
        builder = builder.use_middleware_with(move || Arc::clone(&middleware));
    }
    let host = builder.build();
    let handle = host.server_handle();
    let base = format!("http://{addr}");
    tokio::spawn(async move {
        let _ = host.run_at(&addr).await;
    });
    let client = reqwest::Client::new();
    for _ in 0..100 {
        if client
            .get(format!("{base}/health/live"))
            .send()
            .await
            .is_ok()
        {
            return (base, handle);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

// ---------------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct OkView {
    ok: bool,
}

#[derive(Default, WebxRequestMeta)]
struct OkRequest;

#[get("/mw/ok")]
impl IRequest<OkView> for OkRequest {}

#[derive(Default)]
struct OkHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<OkRequest, OkView> for OkHandler {
    async fn handle(&mut self, _req: OkRequest) -> ActionResult<OkView> {
        Ok(OkView { ok: true })
    }
}

#[derive(Default, WebxRequestMeta)]
struct SlowRequest;

#[get("/mw/slow")]
impl IRequest<OkView> for SlowRequest {}

#[derive(Default)]
struct SlowHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<SlowRequest, OkView> for SlowHandler {
    async fn handle(&mut self, _req: SlowRequest) -> ActionResult<OkView> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(OkView { ok: true })
    }
}

#[derive(Default, WebxRequestMeta)]
struct BoomRequest;

#[get("/mw/boom")]
impl IRequest<OkView> for BoomRequest {}

#[derive(Default)]
struct BoomHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<BoomRequest, OkView> for BoomHandler {
    async fn handle(&mut self, _req: BoomRequest) -> ActionResult<OkView> {
        Err(Error::internal("boom".to_string()))
    }
}

// ---------------------------------------------------------------------------
// Middlewares
// ---------------------------------------------------------------------------

/// Records before/after markers and wraps the rest of the chain.
struct RecordingContinuation {
    log: Arc<Mutex<Vec<&'static str>>>,
}

#[async_trait::async_trait]
impl IMiddleware for RecordingContinuation {
    async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> webx::Result<ControlFlow<()>> {
        Ok(ControlFlow::Continue(()))
    }

    fn continuation(&self) -> bool {
        true
    }

    async fn invoke_next(
        &self,
        ctx: &mut dyn IHttpContext,
        next: webx::NextStage,
    ) -> webx::Result<()> {
        self.log.lock().unwrap().push("continuation-before");
        let result = next.run(ctx).await;
        self.log.lock().unwrap().push("continuation-after");
        result
    }
}

/// Legacy middleware: `invoke` before, `after` on unwind.
struct RecordingLegacy {
    log: Arc<Mutex<Vec<&'static str>>>,
}

#[async_trait::async_trait]
impl IMiddleware for RecordingLegacy {
    async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> webx::Result<ControlFlow<()>> {
        self.log.lock().unwrap().push("legacy-before");
        Ok(ControlFlow::Continue(()))
    }

    async fn after(&self, _ctx: &mut dyn IHttpContext) -> webx::Result<()> {
        self.log.lock().unwrap().push("legacy-after");
        Ok(())
    }
}

/// Continuation middleware that never advances the chain (circuit breaker /
/// short-circuit shape).
struct ShortCircuit {
    handler_calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl IMiddleware for ShortCircuit {
    async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> webx::Result<ControlFlow<()>> {
        self.handler_calls.fetch_add(1, Ordering::SeqCst);
        Ok(ControlFlow::Continue(()))
    }

    fn continuation(&self) -> bool {
        true
    }

    async fn invoke_next(
        &self,
        ctx: &mut dyn IHttpContext,
        _next: webx::NextStage,
    ) -> webx::Result<()> {
        ctx.response_mut().set_status(418);
        Ok(())
    }
}

/// Legacy middleware whose `after` records the response status it observes.
struct StatusRecorder {
    statuses: Arc<Mutex<Vec<u16>>>,
}

#[async_trait::async_trait]
impl IMiddleware for StatusRecorder {
    async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> webx::Result<ControlFlow<()>> {
        Ok(ControlFlow::Continue(()))
    }

    async fn after(&self, ctx: &mut dyn IHttpContext) -> webx::Result<()> {
        self.statuses.lock().unwrap().push(ctx.response().status());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn continuation_and_legacy_stages_compose_in_order() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (base, handle) = spawn_host(vec![
        Arc::new(RecordingContinuation {
            log: Arc::clone(&log),
        }),
        Arc::new(RecordingLegacy {
            log: Arc::clone(&log),
        }),
    ])
    .await;
    // The readiness probe also traverses the middleware pipeline.
    log.lock().unwrap().clear();

    let response = reqwest::get(format!("{base}/mw/ok")).await.unwrap();
    assert_eq!(response.status(), 200);
    let view: OkView = response.json().await.unwrap();
    assert!(view.ok);

    assert_eq!(
        log.lock().unwrap().clone(),
        vec![
            "continuation-before",
            "legacy-before",
            "legacy-after",
            "continuation-after",
        ],
        "onion order must hold across continuation and legacy stages"
    );
    handle.shutdown();
}

#[tokio::test]
async fn continuation_middleware_can_short_circuit_without_running_the_handler() {
    let handler_calls = Arc::new(AtomicUsize::new(0));
    let (base, handle) = spawn_host(vec![Arc::new(ShortCircuit {
        handler_calls: Arc::clone(&handler_calls),
    })])
    .await;

    let response = reqwest::get(format!("{base}/mw/ok")).await.unwrap();
    assert_eq!(response.status(), 418, "short-circuit response is kept");
    // The handler would answer 200; observing 418 proves `next` was never
    // advanced past the middleware.
    assert_eq!(handler_calls.load(Ordering::SeqCst), 0);
    handle.shutdown();
}

#[tokio::test]
async fn timeout_middleware_answers_504_and_cancels_the_handler() {
    let (base, handle) = spawn_host(vec![Arc::new(TimeoutMiddleware::new(
        Duration::from_millis(50),
    ))])
    .await;

    let started = Instant::now();
    let response = reqwest::get(format!("{base}/mw/slow")).await.unwrap();
    assert_eq!(response.status(), 504);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the slow handler must be cancelled, not waited for"
    );
    handle.shutdown();
}

#[tokio::test]
async fn errors_are_converted_before_legacy_after_hooks_run() {
    let statuses = Arc::new(Mutex::new(Vec::new()));
    // The continuation stage forces the continuation executor for the whole
    // pipeline, so the legacy recorder's `after` runs in the continuation path.
    let (base, handle) = spawn_host(vec![
        Arc::new(RecordingContinuation {
            log: Arc::new(Mutex::new(Vec::new())),
        }),
        Arc::new(StatusRecorder {
            statuses: Arc::clone(&statuses),
        }),
    ])
    .await;

    let response = reqwest::get(format!("{base}/mw/boom")).await.unwrap();
    assert_eq!(response.status(), 500);
    assert_eq!(
        statuses.lock().unwrap().last().copied(),
        Some(500),
        "after hooks must observe the real error response"
    );
    handle.shutdown();
}
