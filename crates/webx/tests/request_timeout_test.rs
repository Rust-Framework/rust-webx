//! `Server.RequestTimeoutSeconds`: handler deadline mapped to `504`.

#![allow(non_snake_case)]

use std::time::Duration;

use webx::*;

#[derive(Debug, Default, WebxRequestMeta)]
struct SlowRequest;

#[get("/slow-handler")]
impl IRequest<String> for SlowRequest {}

#[derive(Inject)]
struct SlowHandler;

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<SlowRequest, String> for SlowHandler {
    async fn handle(&mut self, _req: SlowRequest) -> ActionResult<String> {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        Ok("late".into())
    }
}

async fn spawn_host(timeout_seconds: u64) -> (String, ServerHandle) {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("addr").port()
    };
    let addr = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(move |app| {
            app.useOptions(move |o| o.server.request_timeout_seconds = timeout_seconds)
        })
        .build();
    let handle = host.server_handle();
    let run_addr = addr.clone();
    tokio::spawn(async move {
        let _ = host.run_at(&run_addr).await;
    });

    let base = format!("http://{addr}");
    for _ in 0..100 {
        if reqwest::get(format!("{base}/health/live")).await.is_ok() {
            return (base, handle);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

#[tokio::test]
async fn over_budget_handler_returns_504() {
    let (base, handle) = spawn_host(1).await;
    let response = reqwest::get(format!("{base}/slow-handler"))
        .await
        .expect("request completes");
    assert_eq!(response.status().as_u16(), 504);
    let body: serde_json::Value = response.json().await.expect("problem+json");
    assert_eq!(body["status"], 504);
    handle.shutdown();
}

#[tokio::test]
async fn zero_disables_the_inbound_deadline() {
    let (base, handle) = spawn_host(0).await;
    let response = reqwest::get(format!("{base}/slow-handler"))
        .await
        .expect("request completes");
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.text().await.expect("body"), "\"late\"");
    handle.shutdown();
}
