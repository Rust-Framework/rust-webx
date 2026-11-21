//! TLS listener integration: the engine must terminate TLS inside the worker
//! pool (not silently serve cleartext) and still route a normal HTTP/1.1
//! request through the shared pipeline.

use std::net::TcpListener as StdTcpListener;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use webx_core::http::{IHttpContext, CLIENT_CERT_FINGERPRINT_HEADER};
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

struct FingerprintTarget;

#[async_trait::async_trait]
impl RouteTarget for FingerprintTarget {
    async fn handle(&self, ctx: &mut webx_engine::EngineContext) -> webx_core::error::Result<()> {
        let fingerprint = ctx
            .request()
            .header(CLIENT_CERT_FINGERPRINT_HEADER)
            .unwrap_or("missing")
            .to_string();
        ctx.response_mut().set_status(200);
        ctx.response_mut()
            .write_bytes(fingerprint.into_bytes())
            .await
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
async fn mtls_requires_a_client_certificate_and_exposes_its_fingerprint() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    use rcgen::{BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair};
    use rustls::pki_types::PrivateKeyDer;
    use sha2::{Digest as _, Sha256};

    let mut ca_params = CertificateParams::new(Vec::new()).expect("CA params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_key = KeyPair::generate().expect("CA key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("CA cert");
    let ca_der = ca_cert.der().clone();

    let mut server_params =
        CertificateParams::new(vec!["localhost".to_string()]).expect("server params");
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_key = KeyPair::generate().expect("server key");
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .expect("server cert");

    let mut client_params = CertificateParams::new(Vec::new()).expect("client params");
    client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let client_key = KeyPair::generate().expect("client key");
    let client_cert = client_params
        .signed_by(&client_key, &ca_cert, &ca_key)
        .expect("client cert");

    let dir = tempfile::tempdir().expect("temp dir");
    let server_cert_path = dir.path().join("server.pem");
    let server_key_path = dir.path().join("server.key");
    let ca_path = dir.path().join("ca.pem");
    std::fs::write(&server_cert_path, server_cert.pem()).expect("server cert file");
    std::fs::write(&server_key_path, server_key.serialize_pem()).expect("server key file");
    std::fs::write(&ca_path, ca_cert.pem()).expect("ca file");

    let server_tls = webx_engine::tls::build_server_config_with_client_auth(
        server_cert_path.to_str().unwrap(),
        server_key_path.to_str().unwrap(),
        ca_path.to_str().unwrap(),
        webx_core::config::TlsClientAuth::Required,
    )
    .expect("mTLS server config");

    let mut router = Router::new();
    router.insert(HttpMethod::Get, "/fp", Arc::new(FingerprintTarget));
    let provider = rust_dix::ServiceCollection::new()
        .build()
        .expect("provider");
    let (shutdown, _) = tokio::sync::watch::channel(false);
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
        admission: Arc::new(webx_engine::shared::AdmissionStats::default()),
    });
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = Server::new(shared);
    let handle = server.handle();
    let task = tokio::spawn(server.run_listeners(vec![Listener::tls(listener, server_tls)]));

    let mut roots = rustls::RootCertStore::empty();
    roots.add(ca_der.clone()).expect("root cert");
    let mut client_config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_client_auth_cert(
            vec![client_cert.der().clone()],
            PrivateKeyDer::try_from(client_key.serialize_der()).expect("client key der"),
        )
        .expect("client auth");
    client_config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));

    let stream = loop {
        match TcpStream::connect(addr).await {
            Ok(stream) => break stream,
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    };
    let name = rustls::pki_types::ServerName::try_from("localhost").expect("name");
    let mut tls = tokio::time::timeout(
        Duration::from_secs(5),
        connector.connect(name.clone(), stream),
    )
    .await
    .expect("handshake deadline")
    .expect("mTLS handshake");
    tls.write_all(b"GET /fp HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("write");
    let mut response = Vec::new();
    tls.read_to_end(&mut response).await.expect("read");
    let response = String::from_utf8_lossy(&response);
    let expected = Sha256::digest(client_cert.der())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert!(response.contains(&expected), "{response}");

    // Without a client certificate the handshake must fail.
    let mut no_auth_roots = rustls::RootCertStore::empty();
    no_auth_roots.add(ca_der).expect("root cert");
    let mut no_auth = rustls::ClientConfig::builder()
        .with_root_certificates(no_auth_roots)
        .with_no_client_auth();
    no_auth.alpn_protocols = vec![b"http/1.1".to_vec()];
    let no_auth_connector = tokio_rustls::TlsConnector::from(Arc::new(no_auth));
    let stream = TcpStream::connect(addr).await.expect("connect");
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        no_auth_connector.connect(name, stream),
    )
    .await
    .expect("handshake deadline");
    // TLS 1.3 may complete the client side before the server rejects the
    // missing certificate; the request must still never be served.
    if let Ok(mut tls) = result {
        let wrote = tls
            .write_all(b"GET /fp HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .is_ok();
        let mut body = Vec::new();
        let read = tls.read_to_end(&mut body).await;
        assert!(
            !wrote || read.is_err() || body.is_empty(),
            "mTLS must reject a client without a certificate"
        );
    }

    handle.shutdown();
    task.await.expect("server task").expect("server result");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tls_listener_terminates_and_serves_http() {
    let _ = rustls::crypto::ring::default_provider().install_default();
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
        admission: Arc::new(webx_engine::shared::AdmissionStats::default()),
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
