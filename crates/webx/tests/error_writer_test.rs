//! End-to-end test for the pluggable error-response writer.
//!
//! A custom writer takes over a path prefix while every other path keeps the
//! framework's RFC 9457 `problem+json` response.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use webx::*;

struct CustomWriter;

#[async_trait::async_trait]
impl IErrorResponseWriter for CustomWriter {
    fn matches(&self, route_pattern: Option<&str>, path: &str, _accept: Option<&str>) -> bool {
        route_pattern
            .map(|pattern| pattern.starts_with("/custom/"))
            .unwrap_or(false)
            || path.starts_with("/custom/")
    }

    async fn write(&self, ctx: &mut dyn IHttpContext, error: &Error) {
        let status = error.status_code();
        let body = serde_json::json!({
            "custom": true,
            "status": status,
            "detail": error.public_detail(),
        });
        ctx.response_mut().set_status(status);
        ctx.response_mut()
            .set_header("content-type", "application/json");
        let _ = ctx
            .response_mut()
            .write_bytes(serde_json::to_vec(&body).unwrap_or_default())
            .await;
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct CustomBindingRequest {
    #[fromquery]
    page: u32,
}

#[get("/custom/bind")]
impl IRequest<String> for CustomBindingRequest {}

#[derive(Default)]
struct CustomBindingHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<CustomBindingRequest, String> for CustomBindingHandler {
    async fn handle(&mut self, req: CustomBindingRequest) -> ActionResult<String> {
        Ok(format!("page={}", req.page))
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct PlainBindingRequest {
    #[fromquery]
    page: u32,
}

#[get("/plain/bind")]
impl IRequest<String> for PlainBindingRequest {}

#[derive(Default)]
struct PlainBindingHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<PlainBindingRequest, String> for PlainBindingHandler {
    async fn handle(&mut self, req: PlainBindingRequest) -> ActionResult<String> {
        Ok(format!("page={}", req.page))
    }
}

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

async fn spawn_host() -> String {
    let port = find_free_port();
    let addr = format!("127.0.0.1:{port}");
    let base = format!("http://{addr}");

    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_error_writer(Arc::new(CustomWriter))
        .build();

    tokio::spawn(async move {
        let _ = host.run_at(&addr).await;
    });

    for _ in 0..100 {
        if reqwest::get(format!("{base}/health/live")).await.is_ok() {
            return base;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

#[tokio::test]
async fn custom_writer_handles_binding_errors_on_its_prefix() {
    let base = spawn_host().await;

    let response = reqwest::get(format!("{base}/custom/bind?page=abc"))
        .await
        .expect("request succeeds");

    assert_eq!(response.status(), 400);
    assert_eq!(
        response.headers()["content-type"].to_str().unwrap(),
        "application/json",
        "the custom writer owns the envelope"
    );
    let body: serde_json::Value = response.json().await.expect("JSON body");
    assert_eq!(body["custom"], true);
    assert_eq!(body["status"], 400);
}

#[tokio::test]
async fn custom_writer_handles_404s_by_path() {
    let base = spawn_host().await;

    let response = reqwest::get(format!("{base}/custom/missing"))
        .await
        .expect("request succeeds");

    assert_eq!(response.status(), 404);
    let body: serde_json::Value = response.json().await.expect("JSON body");
    assert_eq!(body["custom"], true);
}

#[tokio::test]
async fn other_paths_keep_problem_json() {
    let base = spawn_host().await;

    let response = reqwest::get(format!("{base}/plain/bind?page=abc"))
        .await
        .expect("request succeeds");

    assert_eq!(response.status(), 400);
    assert_eq!(
        response.headers()["content-type"].to_str().unwrap(),
        "application/problem+json"
    );
}
