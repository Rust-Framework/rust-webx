//! A broken custom error writer must not escape the request boundary as a
//! dropped connection; the engine falls back to a fixed 500 problem response.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use webx::*;

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

#[derive(Default, WebxRequestMeta)]
struct FailingRequest;

#[derive(Serialize)]
struct FailingView;

#[get("/panic-writer")]
impl IRequest<FailingView> for FailingRequest {}

#[derive(Default)]
struct FailingHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<FailingRequest, FailingView> for FailingHandler {
    async fn handle(&mut self, _req: FailingRequest) -> ActionResult<FailingView> {
        Err(Error::Internal("intentional".to_string()))
    }
}

struct PanickingWriter;

#[async_trait::async_trait]
impl IErrorResponseWriter for PanickingWriter {
    fn matches(&self, _route: Option<&str>, _path: &str, _accept: Option<&str>) -> bool {
        true
    }

    async fn write(&self, _ctx: &mut dyn IHttpContext, _error: &Error) {
        panic!("error writer panic");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn panicking_error_writer_becomes_a_fallback_500() {
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let base = format!("http://{addr}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_error_writer(Arc::new(PanickingWriter))
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

    let response = client
        .get(format!("{base}/panic-writer"))
        .send()
        .await
        .expect("connection survives the writer panic");
    assert_eq!(response.status().as_u16(), 500);
    let body: serde_json::Value = response.json().await.expect("problem json");
    assert_eq!(body["status"], 500);
}
