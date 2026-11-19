//! End-to-end test for `IHttpRequest::body_stream`.
//!
//! A middleware consumes the live body before the handler; the endpoint itself
//! has no body fields, so binding does not touch the consumed body.

use std::net::TcpListener;
use std::ops::ControlFlow;
use std::time::Duration;

use futures_util::StreamExt;
use webx::*;

#[derive(Debug, Default, WebxRequestMeta)]
struct StreamBodyRequest {}

#[post("/stream-body")]
impl IRequest<String> for StreamBodyRequest {}

#[derive(Default)]
struct StreamBodyHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<StreamBodyRequest, String> for StreamBodyHandler {
    async fn handle(&mut self, _req: StreamBodyRequest) -> ActionResult<String> {
        Ok("ok".to_string())
    }
}

#[derive(Default)]
struct BodyLength;

#[async_trait::async_trait]
impl IMiddleware for BodyLength {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        let mut stream = ctx.request_mut().body_stream().await?;
        let mut length = 0usize;
        while let Some(chunk) = stream.next().await {
            length = length.saturating_add(chunk?.len());
        }
        ctx.response_mut()
            .set_header("x-body-length", &length.to_string());
        Ok(ControlFlow::Continue(()))
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
        .use_middleware::<BodyLength>()
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
async fn middleware_can_stream_the_request_body() {
    let base = spawn_host().await;
    let response = reqwest::Client::new()
        .post(format!("{base}/stream-body"))
        .body("hello stream")
        .send()
        .await
        .expect("request succeeds");

    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-body-length"].to_str().unwrap(), "12");
    let body: String = response.json().await.expect("JSON body");
    assert_eq!(body, "ok");
}
