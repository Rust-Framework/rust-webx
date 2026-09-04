//! Graceful shutdown via ServerHandle and IHost::stop().

use std::net::TcpListener;
use std::sync::Arc;

use rust_webx_core::app::IHost;

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn server_handle_shutdown_stops_run_at() {
    let port = find_free_port();
    let host = rust_webx_host::server::Host::builder()
        .mode(rust_webx_core::mode::AppMode::Development)
        .no_spa()
        .build();
    let handle = host.server_handle();
    let addr = format!("127.0.0.1:{}", port);

    let server = tokio::spawn(async move { host.run_at(&addr).await.unwrap() });

    tokio::time::sleep(tokio::time::Duration::from_millis(400)).await;
    let ok = reqwest::get(format!("http://127.0.0.1:{}/health/live", port))
        .await
        .unwrap();
    assert_eq!(ok.status().as_u16(), 200);

    handle.shutdown();
    server.await.unwrap();
}

#[tokio::test]
async fn ihost_stop_triggers_shutdown() {
    let port = find_free_port();
    let host = Arc::new(
        rust_webx_host::server::Host::builder()
            .mode(rust_webx_core::mode::AppMode::Development)
            .no_spa()
            .build(),
    );
    let addr = format!("127.0.0.1:{}", port);
    let runner = Arc::clone(&host);

    let server = tokio::spawn(async move { runner.run_at(&addr).await.unwrap() });

    tokio::time::sleep(tokio::time::Duration::from_millis(400)).await;
    host.stop().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn integration_payload_too_large_returns_413_problem_json() {
    let port = find_free_port();
    let addr = format!("127.0.0.1:{}", port);
    let host = rust_webx_host::server::Host::builder()
        .mode(rust_webx_core::mode::AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.useOptions(|o| o.app.max_body_size = 64);
        })
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    tokio::time::sleep(tokio::time::Duration::from_millis(400)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/health", port))
        .body(vec![b'x'; 128])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 413);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.contains("application/problem+json"));
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 413);
    assert_eq!(body["title"], "Payload Too Large");

    handle.shutdown();
}
