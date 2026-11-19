//! HTTP/2 integration tests.
//!
//! The server uses a version-adaptive connection builder, so a cleartext
//! listener must accept h2c prior-knowledge and a TLS listener must negotiate
//! h2 through ALPN. These tests assert both, plus that HTTP/1.1 still works.

use std::net::TcpListener;

use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::client::conn::http2;
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start_http_host(port: u16) -> webx_host::server::ServerHandle {
    let url = format!("http://127.0.0.1:{port}");
    let host = webx_host::server::Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .configure(move |app| {
            app.useOptions(move |o| o.app.urls = vec![url.clone()]);
        })
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move { host.run().await.unwrap() });
    handle
}

async fn wait_ready(port: u16) {
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
            return;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }
    panic!("server did not become ready");
}

#[tokio::test]
async fn http2_cleartext_prior_knowledge_is_accepted() {
    let port = find_free_port();
    let handle = start_http_host(port);
    wait_ready(port).await;

    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let (mut send_request, connection) = http2::handshake(TokioExecutor::new(), TokioIo::new(stream))
        .await
        .expect("h2 handshake");
    tokio::spawn(async move {
        let _ = connection.await;
    });

    let request = Request::builder()
        .method("GET")
        .uri("/health/live")
        .body(Empty::<Bytes>::new())
        .expect("request");
    let response = send_request.send_request(request).await.expect("send request");
    assert_eq!(
        response.version(),
        http::Version::HTTP_2,
        "server must answer over HTTP/2"
    );
    assert_eq!(response.status().as_u16(), 200);

    handle.shutdown();
}

#[tokio::test]
async fn http1_still_works_on_the_same_listener() {
    let port = find_free_port();
    let handle = start_http_host(port);
    wait_ready(port).await;

    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://127.0.0.1:{port}/health/live"))
        .send()
        .await
        .expect("h1 request");
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.version(), http::Version::HTTP_11);

    handle.shutdown();
}

#[tokio::test]
async fn http2_over_tls_negotiates_h2_via_alpn() {
    let port = find_free_port();
    let dir = tempfile::tempdir().unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    std::fs::write(&cert_path, cert.cert.pem()).unwrap();
    std::fs::write(&key_path, cert.key_pair.serialize_pem()).unwrap();

    let url = format!("https://127.0.0.1:{port}");
    let cert_s = cert_path.to_string_lossy().into_owned();
    let key_s = key_path.to_string_lossy().into_owned();
    let host = webx_host::server::Host::builder()
        .mode(webx_core::mode::AppMode::Development)
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

    // reqwest with rustls advertises h2 + http/1.1 via ALPN; a compliant server
    // selects h2. A prior-knowledge http2::handshake cannot go through TLS here,
    // so we assert the negotiated version on the response.
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    let response = client
        .get(format!("https://127.0.0.1:{port}/health/live"))
        .send()
        .await
        .expect("https request");
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response.version(),
        http::Version::HTTP_2,
        "TLS ALPN must negotiate HTTP/2"
    );

    handle.shutdown();
}
