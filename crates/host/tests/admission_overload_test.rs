//! Overload protection: when the global connection governor is saturated, a
//! socket that waits past `Server.AdmissionTimeoutMs` gets `503` +
//! `Retry-After` instead of being hidden in the kernel backlog.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use webx_core::mode::AppMode;
use webx_host::server::Host;

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saturated_admission_answers_503_with_retry_after() {
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.options(|options| {
                options.app.max_connections = 1;
                options.server.admission_timeout_ms = 100;
                options.metrics.enabled = true;
            })
        })
        .build();
    let spawn_addr = addr.clone();
    tokio::spawn(async move {
        let _ = host.run_at(&spawn_addr).await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Occupy the only admission slot with a silent connection: it is admitted
    // and then waits for a request (first-request timeout is 10s by default).
    let first = TcpStream::connect(&addr).await.expect("first connect");
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The second connection cannot be admitted within 100ms and must be shed.
    let mut second = TcpStream::connect(&addr).await.expect("second connect");
    second
        .write_all(b"GET /health/live HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .expect("write request");
    let mut buffer = vec![0u8; 2048];
    let read = tokio::time::timeout(Duration::from_secs(3), second.read(&mut buffer))
        .await
        .expect("overload response timeout")
        .expect("read overload response");
    let response = String::from_utf8_lossy(&buffer[..read]);
    assert!(
        response.starts_with("HTTP/1.1 503"),
        "expected a 503 overload response, got: {response}"
    );
    assert!(
        response.to_ascii_lowercase().contains("retry-after: 1"),
        "the overload response must tell the client when to retry: {response}"
    );

    drop(first);

    // The rejection is visible in /metrics once a slot frees up for the scrape.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let metrics = reqwest::get(format!("http://{addr}/metrics"))
        .await
        .expect("metrics request")
        .text()
        .await
        .expect("metrics body");
    assert!(
        metrics.contains("engine_connections_rejected_total 1"),
        "the rejected connection must be counted: {metrics}"
    );
    assert!(
        metrics.contains("engine_connections_admitted_total "),
        "the admitted connections must be counted: {metrics}"
    );
    assert!(
        metrics.contains("engine_admission_waiting 0"),
        "no admission wait should be left behind: {metrics}"
    );
}
