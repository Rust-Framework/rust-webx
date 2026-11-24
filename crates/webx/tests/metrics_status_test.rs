//! Metrics must observe the real error status, not the placeholder `200` that
//! existed before the engine wrote an error response.

use std::net::TcpListener;
use std::time::Duration;

use serde::Serialize;
use std::ops::ControlFlow;
use webx::*;

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

#[derive(Default, WebxRequestMeta)]
struct FailRequest;

#[derive(Serialize)]
struct FailView {
    ok: bool,
}

#[get("/fail")]
impl IRequest<FailView> for FailRequest {}

#[derive(Default)]
struct FailHandler;

#[derive(Default)]
struct PoisonHeaders;

#[async_trait::async_trait]
impl IMiddleware for PoisonHeaders {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        ctx.response_mut()
            .set_header("cache-control", "public, max-age=3600");
        ctx.response_mut().set_header("etag", "\"stale\"");
        Ok(ControlFlow::Continue(()))
    }
}

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<FailRequest, FailView> for FailHandler {
    async fn handle(&mut self, _req: FailRequest) -> ActionResult<FailView> {
        Err(Error::internal("intentional failure".to_string()))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_handler_is_counted_as_5xx_before_after_hooks() {
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let base = format!("http://{addr}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_middleware(MiddlewareSpec::new::<PoisonHeaders>())
        .configure(|app| app.useOptions(|options| options.metrics.enabled = true))
        .build();
    tokio::spawn(async move { host.run_at(&addr).await });

    let client = reqwest::Client::new();
    for _ in 0..100 {
        if client
            .get(format!("{base}/health/live"))
            .send()
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let failed = client.get(format!("{base}/fail")).send().await.unwrap();
    assert_eq!(failed.status().as_u16(), 500);
    assert!(
        failed
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == "no-store"),
        "the error response must replace the representation cache policy"
    );
    assert!(failed.headers().get("etag").is_none());

    let metrics = client
        .get(format!("{base}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        metrics.contains("http_responses_5xx_total 1"),
        "metrics did not observe the handler error: {metrics}"
    );
    // Per-route RED signals (2.2): the failed request is labelled with its
    // matched route pattern and observed in the latency histogram.
    assert!(
        metrics.contains("http_route_requests_total{route=\"/fail\",status=\"5xx\"} 1"),
        "route label missing: {metrics}"
    );
    assert!(
        metrics.contains("http_request_duration_seconds_count{route=\"/fail\"} 1"),
        "route latency histogram missing: {metrics}"
    );
    assert!(
        metrics.contains("http_request_duration_seconds_bucket{route=\"/fail\",le=\"+Inf\"} 1"),
        "histogram +Inf bucket missing: {metrics}"
    );
    // Unmatched paths share one bounded label instead of growing the map.
    assert!(
        metrics.contains("http_requests_inflight"),
        "inflight gauge missing: {metrics}"
    );
}
