//! WEBX-4: the trace id must be visible to handlers, not just the response.
//!
//! `RequestTracing` assigns the request's trace id and writes the `x-trace-id`
//! response header. Before this test existed the id never reached the request
//! context, so a handler could not stamp it onto audit rows/logs and the value
//! it used could disagree with the header the client received.
//!
//! The end-to-end path checked here is the real one: middleware → router →
//! `RequestContext` scope → handler. `RequestContext::trace_id()` inside the
//! handler must equal the response header, including when an upstream
//! `x-request-id` propagated the id.

use std::net::TcpListener;

use serde::{Deserialize, Serialize};
use webx::*;

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Spawn a host whose pipeline includes the tracing middleware and return the
/// base URL once it answers `/health/live`.
async fn spawn_host_with_tracing() -> String {
    let port = find_free_port();
    let addr = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_middleware::<RequestTracing>()
        .build();
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
            return base;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

#[derive(Debug, Serialize, Deserialize)]
struct TraceView {
    trace: Option<String>,
}

#[derive(Default, WebxRequestMeta)]
struct ReadTraceRequest;

#[get("/trace/whoami")]
impl IRequest<TraceView> for ReadTraceRequest {}

#[derive(Default)]
struct ReadTraceHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<ReadTraceRequest, TraceView> for ReadTraceHandler {
    async fn handle(&mut self, _req: ReadTraceRequest) -> ActionResult<TraceView> {
        Ok(TraceView {
            trace: RequestContext::trace_id().map(|id| id.to_string()),
        })
    }
}

#[tokio::test]
async fn handler_reads_the_same_trace_id_as_the_response_header() {
    let base = spawn_host_with_tracing().await;

    let resp = reqwest::get(format!("{base}/trace/whoami")).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    let header = resp
        .headers()
        .get("x-trace-id")
        .expect("RequestTracing must emit x-trace-id")
        .to_str()
        .unwrap()
        .to_string();
    let body: TraceView = resp.json().await.unwrap();

    assert!(
        !header.is_empty(),
        "the tracing middleware must emit a non-empty trace id"
    );
    assert_eq!(
        body.trace.as_deref(),
        Some(header.as_str()),
        "the handler must read the very id the client sees in x-trace-id"
    );
}

#[tokio::test]
async fn upstream_request_id_propagates_to_the_handler() {
    let base = spawn_host_with_tracing().await;

    let resp = reqwest::Client::new()
        .get(format!("{base}/trace/whoami"))
        .header("x-request-id", "upstream-123")
        .send()
        .await
        .unwrap();

    let header = resp
        .headers()
        .get("x-trace-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let body: TraceView = resp.json().await.unwrap();

    assert_eq!(header, "upstream-123");
    assert_eq!(body.trace.as_deref(), Some("upstream-123"));
}
