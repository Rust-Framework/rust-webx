//! End-to-end tests for `OperationResult`.
//!
//! The envelope is a business DTO: the HTTP status stays the framework's
//! (200 for a serialized response, `problem+json` for `Err(webx::Error)`), while
//! `status` is the caller-defined business code carried inside the body.

use std::net::TcpListener;

use serde::Serialize;
use webx::*;

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

async fn spawn_host() -> String {
    let port = find_free_port();
    let addr = format!("127.0.0.1:{port}");
    let host = Host::builder().mode(AppMode::Development).no_spa().build();
    let base = format!("http://{addr}");
    tokio::spawn(async move {
        host.run_at(&addr).await.unwrap();
    });
    for _ in 0..100 {
        if reqwest::get(format!("{base}/health/live")).await.is_ok() {
            return base;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

// ---------------------------------------------------------------------------
// OperationResult<T> — the payload form
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct OrderDto {
    id: String,
    sku: String,
}

#[derive(Default, WebxRequestMeta)]
struct CreateOrderRequest {
    sku: String,
}

#[post("/orders")]
impl IRequest<OperationResult<OrderDto>> for CreateOrderRequest {}

#[derive(Default)]
struct CreateOrderHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<CreateOrderRequest, OperationResult<OrderDto>> for CreateOrderHandler {
    async fn handle(&mut self, req: CreateOrderRequest) -> ActionResult<OperationResult<OrderDto>> {
        Ok(OperationResult::ok(
            OrderDto {
                id: "o-1".into(),
                sku: req.sku,
            },
            "order created",
        ))
    }
}

#[tokio::test]
async fn generic_envelope_is_serialized_with_the_payload() {
    let base = spawn_host().await;

    let resp = reqwest::Client::new()
        .post(format!("{base}/orders"))
        .json(&serde_json::json!({ "sku": "A-1" }))
        .send()
        .await
        .unwrap();

    // A DTO response is a 200; the business status lives in the body.
    assert_eq!(resp.status().as_u16(), 200);
    let content_type = resp.headers()["content-type"].to_str().unwrap().to_owned();
    assert!(
        content_type.starts_with("application/json"),
        "expected JSON, got {content_type}"
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true);
    assert_eq!(body["status"], 0);
    assert_eq!(body["message"], "order created");
    assert_eq!(body["result"]["id"], "o-1");
    assert_eq!(body["result"]["sku"], "A-1");
}

// ---------------------------------------------------------------------------
// OperationResult — the unit form (no `result` member in the body)
// ---------------------------------------------------------------------------

#[derive(Default, WebxRequestMeta)]
struct DeleteOrderRequest {
    #[frompath]
    id: String,
}

#[delete("/orders/{id}")]
impl IRequest<OperationResult> for DeleteOrderRequest {}

#[derive(Default)]
struct DeleteOrderHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<DeleteOrderRequest, OperationResult> for DeleteOrderHandler {
    async fn handle(&mut self, req: DeleteOrderRequest) -> ActionResult<OperationResult> {
        Ok(OperationResult::success(format!(
            "order {} deleted",
            req.id
        )))
    }
}

#[tokio::test]
async fn unit_envelope_omits_the_result_member() {
    let base = spawn_host().await;

    let resp = reqwest::Client::new()
        .delete(format!("{base}/orders/o-1"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true);
    assert_eq!(body["status"], 0);
    assert_eq!(body["message"], "order o-1 deleted");
    assert!(
        body.get("result").is_none(),
        "the unit envelope must not carry `result`: {body}"
    );
}

// ---------------------------------------------------------------------------
// A business failure carried by the envelope is still a 200 response
// ---------------------------------------------------------------------------

#[derive(Default, WebxRequestMeta)]
struct CancelOrderRequest {
    #[frompath]
    id: String,
}

#[post("/orders/{id}/cancel")]
impl IRequest<OperationResult> for CancelOrderRequest {}

#[derive(Default)]
struct CancelOrderHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<CancelOrderRequest, OperationResult> for CancelOrderHandler {
    async fn handle(&mut self, req: CancelOrderRequest) -> ActionResult<OperationResult> {
        Ok(OperationResult::failed(
            1001,
            format!("order {} already shipped", req.id),
        ))
    }
}

#[tokio::test]
async fn business_failure_does_not_become_an_http_error() {
    let base = spawn_host().await;

    let resp = reqwest::Client::new()
        .post(format!("{base}/orders/o-1/cancel"))
        .send()
        .await
        .unwrap();

    // `status` is the business code, never the HTTP status.
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], false);
    assert_eq!(body["status"], 1001);
    assert_eq!(body["message"], "order o-1 already shipped");
    assert!(body.get("result").is_none());
}
