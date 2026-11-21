//! 2.2-5b: dependency/library route allow-list (`HostBuilder::routes_from`) and
//! the inventory scan switch (`use_inventory_routes`).
//!
//! The routes live in two modules so the allow-list can keep one and drop the
//! other; `module_path!()` records the call site, which is what the builder
//! filters on.

use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use webx::*;

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

async fn wait_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not become ready");
}

async fn get(port: u16, path: &str) -> u16 {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connection");
    let request = format!("GET {path} HTTP/1.1\r\nHost: routes\r\nconnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte).await.expect("read header");
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    head.split_whitespace()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

async fn collect(port: u16, host: Host) -> (u16, u16) {
    let address = format!("127.0.0.1:{port}");
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;
    let allowed = get(port, "/allowed/ping").await;
    let blocked = get(port, "/blocked/ping").await;
    handle.shutdown();
    (allowed, blocked)
}

mod allowed {
    use super::*;

    #[derive(Default, WebxRequestMeta)]
    pub struct PingRequest;

    #[derive(serde::Serialize)]
    pub struct PingView {
        pub scope: &'static str,
    }

    #[get("/allowed/ping")]
    impl IRequest<PingView> for PingRequest {}

    #[derive(Default)]
    pub struct PingHandler;

    #[handler]
    #[async_trait::async_trait]
    impl IRequestHandler<PingRequest, PingView> for PingHandler {
        async fn handle(&mut self, _req: PingRequest) -> ActionResult<PingView> {
            Ok(PingView { scope: "allowed" })
        }
    }
}

mod blocked {
    use super::*;

    #[derive(Default, WebxRequestMeta)]
    pub struct PingRequest;

    #[derive(serde::Serialize)]
    pub struct PingView {
        pub scope: &'static str,
    }

    #[get("/blocked/ping")]
    impl IRequest<PingView> for PingRequest {}

    #[derive(Default)]
    pub struct PingHandler;

    #[handler]
    #[async_trait::async_trait]
    impl IRequestHandler<PingRequest, PingView> for PingHandler {
        async fn handle(&mut self, _req: PingRequest) -> ActionResult<PingView> {
            Ok(PingView { scope: "blocked" })
        }
    }
}

#[tokio::test]
async fn routes_from_keeps_only_allowlisted_sources() {
    let port = free_port();
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .routes_from(&["routes_from_test::allowed"])
        .build();
    let (allowed, blocked) = collect(port, host).await;
    assert_eq!(allowed, 200, "the allow-listed route must be served");
    assert_eq!(blocked, 404, "routes outside the allow-list must not exist");
}

#[tokio::test]
async fn default_host_registers_every_compiled_route() {
    let port = free_port();
    let host = Host::builder().mode(AppMode::Development).no_spa().build();
    let (allowed, blocked) = collect(port, host).await;
    assert_eq!(allowed, 200);
    assert_eq!(
        blocked, 200,
        "without a whitelist every compiled route is served"
    );
}

#[tokio::test]
async fn inventory_routes_can_be_disabled_entirely() {
    let port = free_port();
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_inventory_routes(false)
        .build();
    let (allowed, blocked) = collect(port, host).await;
    assert_eq!(
        allowed, 404,
        "scan disabled: no compiled route is registered"
    );
    assert_eq!(blocked, 404);
}
