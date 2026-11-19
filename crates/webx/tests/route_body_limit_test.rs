//! `#[max_body_size(..)]`: per-route body caps become `413` before binding.

#![allow(non_snake_case)]

use std::net::TcpListener;

use webx::*;

#[derive(Debug, Default, WebxRequestMeta)]
struct LimitedRequest {
    #[frombody]
    data: String,
}

#[post("/limited")]
#[max_body_size(64)]
impl IRequest<usize> for LimitedRequest {}

#[derive(Inject)]
struct LimitedHandler;

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<LimitedRequest, usize> for LimitedHandler {
    async fn handle(&mut self, req: LimitedRequest) -> ActionResult<usize> {
        Ok(req.data.len())
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct UnlimitedRequest {
    #[frombody]
    data: String,
}

#[post("/unlimited")]
impl IRequest<usize> for UnlimitedRequest {}

#[derive(Inject)]
struct UnlimitedHandler;

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<UnlimitedRequest, usize> for UnlimitedHandler {
    async fn handle(&mut self, req: UnlimitedRequest) -> ActionResult<usize> {
        Ok(req.data.len())
    }
}

async fn spawn_host() -> (String, ServerHandle) {
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("addr").port()
    };
    let addr = format!("127.0.0.1:{port}");
    let host = Host::builder().mode(AppMode::Development).no_spa().build();
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
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

#[tokio::test]
async fn route_limit_rejects_an_oversized_body_with_413() {
    let (base, handle) = spawn_host().await;
    let oversized = format!("{{\"data\":\"{}\"}}", "x".repeat(100));
    let response = reqwest::Client::new()
        .post(format!("{base}/limited"))
        .header("content-type", "application/json")
        .body(oversized)
        .send()
        .await
        .expect("request completes");
    assert_eq!(response.status().as_u16(), 413);
    handle.shutdown();
}

#[tokio::test]
async fn route_limit_still_accepts_a_small_body() {
    let (base, handle) = spawn_host().await;
    let response = reqwest::Client::new()
        .post(format!("{base}/limited"))
        .header("content-type", "application/json")
        .body("{\"data\":\"ok\"}")
        .send()
        .await
        .expect("request completes");
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.json::<usize>().await.expect("json"), 2);
    handle.shutdown();
}

#[tokio::test]
async fn a_route_without_the_attribute_keeps_the_global_limit() {
    let (base, handle) = spawn_host().await;
    let body = format!("{{\"data\":\"{}\"}}", "x".repeat(100));
    let response = reqwest::Client::new()
        .post(format!("{base}/unlimited"))
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .expect("request completes");
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.json::<usize>().await.expect("json"), 100);
    handle.shutdown();
}
