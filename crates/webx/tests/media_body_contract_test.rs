//! Media-type and body-limit contracts: JSON must not be accepted as
//! `text/plain`, and a per-route cap may legitimately raise the global default.

use std::net::TcpListener;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use webx::*;

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

#[derive(Deserialize, WebxRequestMeta)]
struct JsonRequest {
    #[frombody]
    message: String,
}

#[derive(Serialize)]
struct JsonView {
    len: usize,
}

#[post("/json")]
impl IRequest<JsonView> for JsonRequest {}

#[derive(Default)]
struct JsonHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<JsonRequest, JsonView> for JsonHandler {
    async fn handle(&mut self, req: JsonRequest) -> ActionResult<JsonView> {
        Ok(JsonView {
            len: req.message.len(),
        })
    }
}

#[derive(Deserialize, WebxRequestMeta)]
struct LargeRequest {
    #[frombody]
    data: String,
}

#[derive(Serialize)]
struct LargeView {
    len: usize,
}

#[post("/large")]
#[max_body_size(64 * 1024 * 1024)]
impl IRequest<LargeView> for LargeRequest {}

#[derive(Default)]
struct LargeHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<LargeRequest, LargeView> for LargeHandler {
    async fn handle(&mut self, req: LargeRequest) -> ActionResult<LargeView> {
        Ok(LargeView {
            len: req.data.len(),
        })
    }
}

async fn spawn_host() -> String {
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let base = format!("http://{addr}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| app.useOptions(|options| options.app.max_body_size = 64))
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
            return base;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

#[tokio::test]
async fn json_body_rejects_a_non_json_media_type() {
    let base = spawn_host().await;
    let response = reqwest::Client::new()
        .post(format!("{base}/json"))
        .header("content-type", "text/plain")
        .body(r#"{"message":"hello"}"#)
        .send()
        .await
        .expect("request");
    assert_eq!(response.status().as_u16(), 415);
}

#[tokio::test]
async fn json_body_accepts_application_json() {
    let base = spawn_host().await;
    let response = reqwest::Client::new()
        .post(format!("{base}/json"))
        .json(&serde_json::json!({ "message": "hello" }))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status().as_u16(), 200);
}

#[tokio::test]
async fn route_body_limit_may_raise_the_global_default() {
    let base = spawn_host().await;
    let payload = format!(r#"{{"data":"{}"}}"#, "x".repeat(64));
    let response = reqwest::Client::new()
        .post(format!("{base}/large"))
        .json(&serde_json::json!({ "data": payload }))
        .send()
        .await
        .expect("request");
    assert_eq!(
        response.status().as_u16(),
        200,
        "a route limit above App.MaxBodySize must be effective for Content-Length bodies"
    );
}
