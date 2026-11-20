//! Host lifecycle invariants: one run per Host, shutdown completes, and an
//! already-disposed host cannot silently advertise a live engine.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use webx_core::mode::AppMode;
use webx_host::server::Host;

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

async fn wait_live(base: &str) {
    let client = reqwest::Client::new();
    for _ in 0..100 {
        if client
            .get(format!("{base}/health/live"))
            .send()
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_run_is_single_use() {
    let host = Arc::new(Host::builder().mode(AppMode::Development).no_spa().build());
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let base = format!("http://{addr}");
    let running = {
        let host = Arc::clone(&host);
        let addr = addr.clone();
        tokio::spawn(async move { host.run_at(&addr).await })
    };
    wait_live(&base).await;

    host.server_handle().shutdown();
    running
        .await
        .expect("run task")
        .expect("first run succeeds");

    let error = host
        .run_at(&format!("127.0.0.1:{}", free_port()))
        .await
        .expect_err("a stopped host is not silently reusable");
    assert!(
        error.to_string().contains("single-use"),
        "expected a typed lifecycle error, got: {error}"
    );
}
