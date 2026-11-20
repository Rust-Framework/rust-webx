//! TLS listener integration: the engine must terminate TLS inside the worker
//! pool (not silently serve cleartext) and still route a normal HTTP/1.1
//! request through the shared pipeline.

use std::net::TcpListener as StdTcpListener;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use webx_core::http::IHttpContext;
use webx_core::routing::HttpMethod;
use webx_engine::{
    BodyLimits, EngineConfig, Listener, Pipeline, RouteTarget, Router, Server, Shared,
};

struct OkTarget;

#[async_trait::async_trait]
impl RouteTarget for OkTarget {
    async fn handle(&self, ctx: &mut webx_engine::EngineContext) -> webx_core::error::Result<()> {
        ctx.response_mut().set_status(200);
        ctx.response_mut().write_bytes(b"secure-ok".to_vec()).await
    }
}

fn config() -> EngineConfig {
    EngineConfig {
        workers: 1,
        max_connections: 8,
        first_request_timeout: Duration::from_secs(5),
        ..EngineConfig::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tls_listener_terminates_and_serves_http() {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("self-signed certificate");
    let cert_der = certified.cert.der().clone();
    let dir = tempfile::tempdir().expect("temp dir");
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, certified.cert.pem()).expect("cert");
    std::fs::write(&key_path, certified.key_pair.serialize_pem()).expect("key");

    let mut router = Router::new();
    router.insert(HttpMethod::Get, "/", Arc::new(OkTarget));
    let provider = rust_dix::ServiceCollection::new()
        .build()
        .expect("provider");
    let (shutdown, _) = tokio::sync::watch::channel(false);
    let server_tls = webx_engine::tls::build_server_config(
        cert_path.to_str().unwrap(),
        key_path.to_str().unwrap(),
    )
    .expect("server tls");
    let shared = Arc::new(Shared {
        config: Arc::new(config()),
        router: Arc::new(router),
        pipeline: Arc::new(Pipeline::new(Vec::new())),
        dispatch: Arc::new(webx_core::DispatchRuntime::new(
            provider,
            Arc::new(webx_core::route::scan::HandlerCache::build()),
        )),
        body_limits: Arc::new(BodyLimits::default()),
        error_writers: Arc::new(Vec::new()),
        tls: Some(Arc::clone(&server_tls)),
        shutdown,
        shutdown_token: webx_core::hosted::CancellationToken::new(),
        immediate_shutdown: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        immediate_watch: tokio::sync::watch::channel(false).0,
        started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = Server::new(shared);
    let handle = server.handle();
    let task = tokio::spawn(server.run_listeners(vec![Listener::tls(listener, server_tls)]));

    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert_der).expect("root cert");
    let mut client = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    client.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client));

    let stream = loop {
        match TcpStream::connect(addr).await {
            Ok(stream) => break stream,
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    };
    let name = rustls::pki_types::ServerName::try_from("localhost").expect("server name");
    let mut tls = tokio::time::timeout(Duration::from_secs(5), connector.connect(name, stream))
        .await
        .expect("tls handshake deadline")
        .expect("tls handshake");
    tls.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("write");
    let mut response = Vec::new();
    tls.read_to_end(&mut response).await.expect("read");
    let response = String::from_utf8_lossy(&response);
    assert!(response.contains("200"), "{response}");
    assert!(response.contains("secure-ok"), "{response}");

    handle.shutdown();
    task.await.expect("server task").expect("server result");
}
