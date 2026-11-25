//! 2.2-1b: connection/queue gauges on `/metrics`.
//!
//! `engine_connections_active` counts open connections (one admission permit
//! each, keep-alive and streaming included), `engine_connections_capacity` is
//! the configured cap, and `engine_admission_waiting` is the shed queue.

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

/// Scrape `/metrics` and return the whole body.
async fn scrape(port: u16) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("metrics connection");
    let request = "GET /metrics HTTP/1.1\r\nHost: gauges\r\nconnection: close\r\n\r\n";
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.expect("read");
    String::from_utf8_lossy(&response).into_owned()
}

fn metric_value(body: &str, name: &str) -> Option<f64> {
    body.lines()
        .find_map(|line| line.strip_prefix(&format!("{name} ")))
        .and_then(|value| value.trim().parse().ok())
}

#[tokio::test]
async fn metrics_report_active_connections_and_capacity() {
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.options(|options| {
                options.metrics.enabled = true;
                options.app.max_connections = 8;
            })
        })
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    // One idle keep-alive connection; the scrape itself is a second one.
    let idle = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("idle connection");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let body = scrape(port).await;
    assert_eq!(
        metric_value(&body, "engine_connections_capacity"),
        Some(8.0),
        "capacity must mirror App.MaxConnections:\n{body}"
    );
    let active = metric_value(&body, "engine_connections_active").expect("active gauge");
    assert!(
        (2.0..=8.0).contains(&active),
        "idle + scraping connections should be active, got {active}:\n{body}"
    );
    assert!(
        metric_value(&body, "engine_admission_waiting").is_some(),
        "queue depth gauge must be present"
    );

    drop(idle);
    handle.shutdown();
}
