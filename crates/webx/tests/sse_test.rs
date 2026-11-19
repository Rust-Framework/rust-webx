//! End-to-end tests for `SseResult` over a real HTTP host.
//!
//! The unit tests in `webx_core::stream` cover encoding and lifecycle in
//! isolation; these tests prove the transport behaviour a browser or SDK sees:
//! SSE headers, chunked delivery without buffering, and the disconnect hook.

#![allow(non_snake_case)] // `#[derive(Inject)]` generates `__rdi_construct_<Type>` helpers.

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use webx::*;

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local_addr")
        .port()
}

/// Per-host record of how each SSE stream ended.
#[derive(Clone, Default)]
struct CloseLog(Arc<Mutex<Vec<&'static str>>>);

impl CloseLog {
    fn record(&self, end: StreamEnd) {
        let kind = match end {
            StreamEnd::Completed => "completed",
            StreamEnd::ClientDisconnected => "client-disconnected",
            StreamEnd::Failed(_) => "failed",
            StreamEnd::Deadline => "deadline",
            StreamEnd::ServerShutdown => "server-shutdown",
        };
        self.0.lock().expect("close log lock").push(kind);
    }

    fn snapshot(&self) -> Vec<&'static str> {
        self.0.lock().expect("close log lock").clone()
    }
}

async fn wait_for_close(log: &CloseLog, expected: &str) {
    for _ in 0..150 {
        if log.snapshot().contains(&expected) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "close hook did not report {expected:?}; observed {:?}",
        log.snapshot()
    );
}

async fn spawn_host() -> (String, Arc<CloseLog>) {
    let log = Arc::new(CloseLog::default());
    let port = find_free_port();
    let addr = format!("127.0.0.1:{port}");
    let base = format!("http://{addr}");

    let registration_log = Arc::clone(&log);
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(move |svc| svc.instance(Arc::clone(&registration_log)))
        .build();

    tokio::spawn(async move {
        let _ = host.run_at(&addr).await;
    });

    for _ in 0..100 {
        if reqwest::get(format!("{base}/health/live")).await.is_ok() {
            return (base, log);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

/// A host that keeps a shutdown handle so a test can drain it mid-stream.
async fn spawn_host_with_handle() -> (String, Arc<CloseLog>, ServerHandle) {
    let log = Arc::new(CloseLog::default());
    let port = find_free_port();
    let addr = format!("127.0.0.1:{port}");
    let base = format!("http://{addr}");

    let registration_log = Arc::clone(&log);
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(move |svc| svc.instance(Arc::clone(&registration_log)))
        .build();
    let server = host.into_server();
    let handle = server.handle();

    tokio::spawn(async move {
        let _ = server.run_at(&addr).await;
    });

    for _ in 0..100 {
        if reqwest::get(format!("{base}/health/live")).await.is_ok() {
            return (base, log, handle);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

// ---------------------------------------------------------------------------
// Routes under test
// ---------------------------------------------------------------------------

#[derive(Debug, Default, WebxRequestMeta)]
struct FiniteRequest;

#[get("/sse/finite")]
impl IRequest<Response> for FiniteRequest {}

#[derive(Inject)]
struct FiniteHandler {
    #[inject]
    log: Arc<CloseLog>,
}

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<FiniteRequest, Response> for FiniteHandler {
    async fn handle(&mut self, _req: FiniteRequest) -> ActionResult<Response> {
        let log = Arc::clone(&self.log);
        let events = futures_util::stream::iter([
            Ok(SseEvent::event("greeting", "hello")),
            Ok(SseEvent::data("world")),
        ]);
        Ok(Box::new(
            SseResult::new(events).on_close(move |end| log.record(end)),
        ))
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct TicksRequest;

#[get("/sse/ticks")]
impl IRequest<Response> for TicksRequest {}

#[derive(Inject)]
struct TicksHandler {
    #[inject]
    log: Arc<CloseLog>,
}

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<TicksRequest, Response> for TicksHandler {
    async fn handle(&mut self, _req: TicksRequest) -> ActionResult<Response> {
        let log = Arc::clone(&self.log);
        let (tx, rx) = tokio::sync::mpsc::channel::<SseEvent>(4);
        tokio::spawn(async move {
            let _ = tx.send(SseEvent::data("one")).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = tx.send(SseEvent::data("two")).await;
        });

        let events = futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (Ok(event), rx))
        });
        Ok(Box::new(
            SseResult::new(events).on_close(move |end| log.record(end)),
        ))
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct EndlessRequest;

#[get("/sse/endless")]
impl IRequest<Response> for EndlessRequest {}

#[derive(Inject)]
struct EndlessHandler {
    #[inject]
    log: Arc<CloseLog>,
}

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<EndlessRequest, Response> for EndlessHandler {
    async fn handle(&mut self, _req: EndlessRequest) -> ActionResult<Response> {
        let log = Arc::clone(&self.log);
        let (tx, rx) = tokio::sync::mpsc::channel::<SseEvent>(4);
        tokio::spawn(async move {
            let mut tick = 0u64;
            loop {
                if tx
                    .send(SseEvent::data(format!("tick {tick}")))
                    .await
                    .is_err()
                {
                    break;
                }
                tick += 1;
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        });

        let events = futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (Ok(event), rx))
        });
        Ok(Box::new(
            SseResult::new(events).on_close(move |end| log.record(end)),
        ))
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct ShutdownAwareRequest;

#[get("/sse/shutdown-aware")]
impl IRequest<Response> for ShutdownAwareRequest {}

#[derive(Inject)]
struct ShutdownAwareHandler {
    #[inject]
    log: Arc<CloseLog>,
    #[inject]
    shutdown: Arc<ShutdownToken>,
}

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<ShutdownAwareRequest, Response> for ShutdownAwareHandler {
    async fn handle(&mut self, _req: ShutdownAwareRequest) -> ActionResult<Response> {
        let log = Arc::clone(&self.log);
        let (tx, rx) = tokio::sync::mpsc::channel::<SseEvent>(4);
        tokio::spawn(async move {
            let mut tick = 0u64;
            loop {
                if tx
                    .send(SseEvent::data(format!("tick {tick}")))
                    .await
                    .is_err()
                {
                    break;
                }
                tick += 1;
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        });

        let events = futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (Ok(event), rx))
        });
        Ok(Box::new(
            SseResult::new(events)
                .on_server_shutdown(self.shutdown.token().clone())
                .on_close(move |end| log.record(end)),
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn finite_stream_sets_sse_headers_and_dispatches_events() {
    let (base, log) = spawn_host().await;

    let resp = reqwest::get(format!("{base}/sse/finite"))
        .await
        .expect("request succeeds");

    assert_eq!(resp.status().as_u16(), 200);
    let headers = resp.headers();
    assert_eq!(
        headers["content-type"], "text/event-stream; charset=utf-8",
        "SSE endpoints must declare the event-stream media type"
    );
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["x-accel-buffering"], "no");
    assert!(
        headers.get("content-length").is_none(),
        "an event stream must use chunked transfer encoding"
    );
    assert!(
        headers.get("content-encoding").is_none(),
        "an event stream must never be compressed"
    );

    let body = resp.text().await.expect("body reads");
    assert_eq!(body, "event:greeting\ndata:hello\n\ndata:world\n\n");

    wait_for_close(&log, "completed").await;
}

#[tokio::test]
async fn ticks_are_flushed_as_they_are_produced() {
    let (base, _log) = spawn_host().await;

    let mut resp = reqwest::get(format!("{base}/sse/ticks"))
        .await
        .expect("request succeeds");

    let first = tokio::time::timeout(Duration::from_secs(2), resp.chunk())
        .await
        .expect("first frame arrives")
        .expect("body reads")
        .expect("first frame is present");
    let first = std::str::from_utf8(&first).expect("frame is UTF-8");
    assert!(first.contains("data:one"), "first frame: {first:?}");
    assert!(
        !first.contains("data:two"),
        "a buffered response would deliver both frames at once: {first:?}"
    );

    let second = tokio::time::timeout(Duration::from_secs(2), resp.chunk())
        .await
        .expect("second frame arrives")
        .expect("body reads")
        .expect("second frame is present");
    assert!(std::str::from_utf8(&second)
        .expect("frame is UTF-8")
        .contains("data:two"));
}

#[tokio::test]
async fn client_disconnect_fires_the_close_hook() {
    let (base, log) = spawn_host().await;

    let mut resp = reqwest::get(format!("{base}/sse/endless"))
        .await
        .expect("request succeeds");
    let first = tokio::time::timeout(Duration::from_secs(2), resp.chunk())
        .await
        .expect("first frame arrives")
        .expect("body reads")
        .expect("first frame is present");
    assert!(std::str::from_utf8(&first)
        .expect("frame is UTF-8")
        .contains("data:tick"));

    // Dropping the response aborts the body; the host must observe the drop and
    // report `ClientDisconnected` so usage settlement can release the stream.
    drop(resp);

    wait_for_close(&log, "client-disconnected").await;
}

#[tokio::test]
async fn graceful_shutdown_ends_the_stream_with_a_server_shutdown_event() {
    let (base, log, handle) = spawn_host_with_handle().await;

    let mut resp = reqwest::get(format!("{base}/sse/shutdown-aware"))
        .await
        .expect("request succeeds");
    let first = tokio::time::timeout(Duration::from_secs(2), resp.chunk())
        .await
        .expect("first frame arrives")
        .expect("body reads")
        .expect("first frame is present");
    let mut seen = String::from_utf8_lossy(&first).to_string();
    assert!(seen.contains("data:tick"), "first frame: {seen:?}");

    // Draining the host cancels the injected `ShutdownToken`; the stream must
    // write a final `server_shutdown` frame and close, not hang until the drain
    // timeout aborts the connection.
    handle.shutdown();
    while !seen.contains("event:server_shutdown") {
        let Some(chunk) = tokio::time::timeout(Duration::from_secs(3), resp.chunk())
            .await
            .expect("shutdown frame arrives before the drain timeout")
            .expect("body reads")
        else {
            break;
        };
        seen.push_str(&String::from_utf8_lossy(&chunk));
    }

    assert!(
        seen.contains("event:server_shutdown"),
        "the client must be told why the stream ended: {seen:?}"
    );
    assert!(seen.contains("data:{}"), "empty shutdown payload: {seen:?}");
    wait_for_close(&log, "server-shutdown").await;
}
