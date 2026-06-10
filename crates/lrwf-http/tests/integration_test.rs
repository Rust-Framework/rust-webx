/// Integration tests for LRWF host.
///
/// These tests spin up a minimal LRWF host and verify full HTTP cycles.

use std::net::TcpListener;

async fn spawn_test_host(port: u16) {
    let addr = format!("127.0.0.1:{}", port);
    let host = lrwf_http::server::Host::builder()
        .mode(lrwf_core::mode::AppMode::Development)
        .build();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
}

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[tokio::test]
async fn integration_404_for_unregistered_route() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(&format!("http://127.0.0.1:{}/nonexistent", port))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 404);
    assert!(body["error"].as_str().unwrap().contains("Not Found"));
}

#[tokio::test]
async fn integration_health_check_openapi_available() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(&format!("http://127.0.0.1:{}/api/openapi.html", port))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let content_type = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(content_type.contains("text/html"));
}

#[tokio::test]
async fn integration_openapi_json_available() {
    let port = find_free_port();
    spawn_test_host(port).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(&format!("http://127.0.0.1:{}/api/openapi.json", port))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("openapi").is_some());
    assert!(body.get("info").is_some());
}
