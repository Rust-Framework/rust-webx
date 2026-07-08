//! Integration test helpers — spawn host on ephemeral port.

use std::net::TcpListener;
use std::time::Duration;

use crate::server::{Host, ServerHandle};

/// Pick a free TCP port on localhost.
pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local_addr")
        .port()
}

/// Poll until `GET {base}{health_path}` returns success.
pub async fn wait_until_ready(base_url: &str, health_path: &str) {
    let url = format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        health_path.trim_start_matches('/')
    );
    for _ in 0..60 {
        if let Ok(resp) = reqwest::get(&url).await {
            if resp.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("server failed to become ready at {url}");
}

/// Running test server instance.
pub struct TestServer {
    pub base_url: String,
    pub handle: ServerHandle,
    pub task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    pub async fn teardown(self) {
        self.handle.shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.task).await;
    }
}

/// Build host, bind to `127.0.0.1:{port}`, wait for health, return client base URL.
pub async fn spawn(host: Host, port: u16) -> TestServer {
    spawn_with_health(host, port, "/health/live").await
}

/// Same as [`spawn`] with a custom readiness probe path.
pub async fn spawn_with_health(host: Host, port: u16, health_path: &str) -> TestServer {
    let addr = format!("127.0.0.1:{port}");
    let base_url = format!("http://{addr}");
    let handle = host.server_handle();
    let task = tokio::spawn(async move {
        let _ = host.run_at(&addr).await;
    });
    wait_until_ready(&base_url, health_path).await;
    TestServer {
        base_url,
        handle,
        task,
    }
}
