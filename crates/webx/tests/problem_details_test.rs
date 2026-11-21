//! Error 2.0: the default `problem+json` body carries a stable machine-readable
//! `code`, `retryable`, the request `instance` and the `traceId` extension.

use std::net::TcpListener;
use std::time::Duration;

use serde::Serialize;
use webx::*;

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

#[derive(Default, WebxRequestMeta)]
struct BoomRequest;

#[derive(Serialize)]
struct BoomView {
    ok: bool,
}

#[get("/boom")]
impl IRequest<BoomView> for BoomRequest {}

#[derive(Default)]
struct BoomHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<BoomRequest, BoomView> for BoomHandler {
    async fn handle(&mut self, _req: BoomRequest) -> ActionResult<BoomView> {
        Err(Error::Internal(
            "connection string must stay in logs".into(),
        ))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn problem_json_carries_code_retryable_instance_and_trace() {
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let base = format!("http://{addr}");
    let host = Host::builder().mode(AppMode::Development).no_spa().build();
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
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let response = client.get(format!("{base}/boom")).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 500);
    let trace_id = response
        .headers()
        .get("x-trace-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body: serde_json::Value = response.json().await.unwrap();

    assert_eq!(body["status"], 500);
    assert_eq!(body["title"], "Internal Server Error");
    assert_eq!(body["code"], "internal_error");
    assert_eq!(body["retryable"], false);
    assert_eq!(body["instance"], "/boom");
    assert!(
        !body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("connection string"),
        "internal detail must not leak: {body}"
    );
    if let Some(trace_id) = trace_id {
        assert_eq!(body["traceId"], trace_id);
    }

    // Unmatched routes get the same shape, derived from the 404 status.
    let missing = client
        .get(format!("{base}/definitely-missing"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status().as_u16(), 404);
    let body: serde_json::Value = missing.json().await.unwrap();
    assert_eq!(body["code"], "not_found");
    assert_eq!(body["retryable"], false);
    assert_eq!(body["instance"], "/definitely-missing");
}
