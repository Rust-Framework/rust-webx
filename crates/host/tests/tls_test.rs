//! TLS (HTTPS) integration tests.

use std::net::TcpListener;

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn integration_https_health_with_generated_cert() {
    let port = find_free_port();
    let dir = tempfile::tempdir().unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");

    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    std::fs::write(&cert_path, cert.cert.pem()).unwrap();
    std::fs::write(&key_path, cert.key_pair.serialize_pem()).unwrap();

    let url = format!("https://127.0.0.1:{}", port);
    let cert_s = cert_path.to_string_lossy().into_owned();
    let key_s = key_path.to_string_lossy().into_owned();

    let host = rust_webx_host::server::Host::builder()
        .mode(rust_webx_core::mode::AppMode::Development)
        .no_spa()
        .configure(move |app| {
            app.useOptions(move |o| {
                o.app.urls = vec![url.clone()];
                o.tls.cert_path = cert_s.clone();
                o.tls.key_path = key_s.clone();
            });
        })
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move { host.run().await.unwrap() });
    tokio::time::sleep(tokio::time::Duration::from_millis(600)).await;

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    let resp = client
        .get(format!("https://127.0.0.1:{}/health/live", port))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "pass");

    handle.shutdown();
}

#[test]
fn build_tls_acceptor_rejects_missing_cert() {
    let err = rust_webx_host::server::Host::builder()
        .mode(rust_webx_core::mode::AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.useOptions(|o| {
                o.app.urls = vec!["https://127.0.0.1:8443".into()];
                o.tls.cert_path = "/nonexistent/cert.pem".into();
                o.tls.key_path = "/nonexistent/key.pem".into();
            });
        })
        .build();

    // build() succeeds; TLS error surfaces at run() when binding HTTPS
    let host = err;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(host.run());
    assert!(result.is_err());
}
