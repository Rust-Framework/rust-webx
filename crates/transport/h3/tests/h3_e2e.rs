//! End-to-end HTTP/3 test for the 2.9 M3 transport adapter.
//!
//! Everything here is real on the wire: a quinn UDP endpoint with a rustls TLS
//! 1.3 handshake (ALPN `h3`), an `h3` client, a `quinn` client endpoint, and the
//! host's production engine pipeline behind the transport. The only test
//! affordance is a certificate verifier that accepts the test's self-signed
//! certificate (the same approach `rcgen`-based rustls tests use upstream).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf as _, BufMut as _, Bytes, BytesMut};
use futures_util::io::AsyncReadExt as _;
use futures_util::{SinkExt, StreamExt};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use webx::*;
use webx_core::config::TlsClientAuth;
use webx_engine::IHttpTransport;
use webx_h3::webtransport::{IWebTransportHandler, WebTransportServerSession};
use webx_h3::{H3Config, H3Transport};

/// Generous upper bound so a stuck transport fails the test instead of hanging
/// the CI job; normal runs finish in well under a second.
const TEST_TIMEOUT: Duration = Duration::from_secs(20);

// ---------------------------------------------------------------------------
// Routes: defined once, registered by inventory, served through the real engine
// ---------------------------------------------------------------------------

#[derive(Debug, Default, WebxRequestMeta)]
struct H3HelloRequest;

#[get("/h3/hello")]
impl IRequest<H3HelloReply> for H3HelloRequest {}

#[derive(Debug, Serialize)]
struct H3HelloReply {
    message: &'static str,
}

#[derive(Default)]
struct H3HelloHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<H3HelloRequest, H3HelloReply> for H3HelloHandler {
    async fn handle(&mut self, _req: H3HelloRequest) -> ActionResult<H3HelloReply> {
        Ok(H3HelloReply {
            message: "hello h3",
        })
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct H3EchoRequest {
    #[frombody]
    data: String,
}

#[post("/h3/echo")]
impl IRequest<String> for H3EchoRequest {}

#[derive(Default)]
struct H3EchoHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<H3EchoRequest, String> for H3EchoHandler {
    async fn handle(&mut self, req: H3EchoRequest) -> ActionResult<String> {
        Ok(req.data)
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct H3SlowRequest;

#[get("/h3/slow")]
impl IRequest<String> for H3SlowRequest {}

#[derive(Default)]
struct H3SlowHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<H3SlowRequest, String> for H3SlowHandler {
    async fn handle(&mut self, _req: H3SlowRequest) -> ActionResult<String> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok("slow done".to_string())
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct H3SseRequest;

#[get("/h3/sse")]
impl IRequest<Response> for H3SseRequest {}

#[derive(Default)]
struct H3SseHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<H3SseRequest, Response> for H3SseHandler {
    async fn handle(&mut self, _req: H3SseRequest) -> ActionResult<Response> {
        let events =
            futures_util::stream::iter([Ok(SseEvent::data("first")), Ok(SseEvent::data("second"))]);
        Ok(Box::new(SseResult::new(events)))
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct H3CertRequest {
    #[fromheader("x-webx-client-cert-sha256")]
    certificate: Option<String>,
}

#[get("/h3/cert")]
impl IRequest<String> for H3CertRequest {}

#[derive(Default)]
struct H3CertHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<H3CertRequest, String> for H3CertHandler {
    async fn handle(&mut self, req: H3CertRequest) -> ActionResult<String> {
        Ok(req.certificate.unwrap_or_default())
    }
}

/// WebTransport echo handler: accepts bidirectional streams and echoes their
/// bytes back on the same stream (adapter-level WebTransport MVP).
struct H3WebTransportEcho;

fn wt_error(error: impl std::fmt::Display) -> webx::Error {
    webx::Error::bad_request(format!("webtransport: {error}"))
}

#[async_trait::async_trait]
impl IWebTransportHandler for H3WebTransportEcho {
    async fn authorize(
        &self,
        _info: &webx_h3::webtransport::WebTransportRequestInfo,
    ) -> Result<()> {
        // The port is fail-closed: a handler that wants traffic must opt in.
        Ok(())
    }

    async fn on_session(
        &self,
        _info: webx_h3::webtransport::WebTransportRequestInfo,
        session: Arc<WebTransportServerSession>,
    ) -> Result<()> {
        loop {
            let Some(resolver) = session.accept_bi().await.map_err(wt_error)? else {
                return Ok(());
            };
            match resolver.resolve().await.map_err(wt_error)? {
                h3_webtransport::server::AcceptedBi::BidiStream(_, stream) => {
                    tokio::spawn(async move {
                        let (mut read, mut write) = stream.split();
                        let _ = futures_util::io::copy(&mut read, &mut write).await;
                    });
                }
                h3_webtransport::server::AcceptedBi::Request(_, response) => {
                    // This MVP serves streams only; HTTP requests on a WebTransport
                    // session are not part of the echo.
                    drop(response);
                }
            }
        }
    }
}

/// Rejects every WebTransport session in the pre-accept authorization hook.
struct H3WebTransportDeny;

#[async_trait::async_trait]
impl IWebTransportHandler for H3WebTransportDeny {
    async fn authorize(
        &self,
        _info: &webx_h3::webtransport::WebTransportRequestInfo,
    ) -> Result<()> {
        Err(webx::Error::bad_request("denied by policy"))
    }

    async fn on_session(
        &self,
        _info: webx_h3::webtransport::WebTransportRequestInfo,
        _session: Arc<WebTransportServerSession>,
    ) -> Result<()> {
        Ok(())
    }
}

/// Does not override `authorize`; the port's fail-closed default must deny it.
struct H3WebTransportNoAuthorize;

#[async_trait::async_trait]
impl IWebTransportHandler for H3WebTransportNoAuthorize {
    async fn on_session(
        &self,
        _info: webx_h3::webtransport::WebTransportRequestInfo,
        _session: Arc<WebTransportServerSession>,
    ) -> Result<()> {
        Ok(())
    }
}

/// WebSocket echo handler served over both h1/h2 and (in this crate) HTTP/3.
struct H3WebSocketEcho;

#[async_trait::async_trait]
impl IWebSocketHandler for H3WebSocketEcho {
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()> {
        while let Some(message) = connection.recv().await {
            match message? {
                RealtimeMessage::Text(text) => connection.send_text(text).await?,
                RealtimeMessage::Binary(bytes) => connection.send_binary(bytes).await?,
                RealtimeMessage::Close { .. } => break,
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// TLS material (CA + server/client certificates)
// ---------------------------------------------------------------------------

struct TlsMaterial {
    _directory: tempfile::TempDir,
    cert: PathBuf,
    key: PathBuf,
    client_ca: PathBuf,
    client_cert: PathBuf,
    client_key: PathBuf,
    client_cert_der: Vec<u8>,
}

fn self_signed_tls() -> TlsMaterial {
    let directory = tempfile::tempdir().expect("temporary directory for the test certificate");

    // Test CA; the server certificate and the client certificate are both
    // signed by it, so `WebPkiClientVerifier` can validate the client chain.
    let mut ca_params = rcgen::CertificateParams::new(vec!["webx-h3-test-ca".to_string()])
        .expect("CA certificate params");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_key = rcgen::KeyPair::generate().expect("CA key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("CA certificate");

    let server_params = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .expect("server certificate params");
    let server_key = rcgen::KeyPair::generate().expect("server key");
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .expect("server certificate");

    let client_params = rcgen::CertificateParams::new(vec!["webx-h3-client".to_string()])
        .expect("client certificate params");
    let client_key = rcgen::KeyPair::generate().expect("client key");
    let client_cert = client_params
        .signed_by(&client_key, &ca_cert, &ca_key)
        .expect("client certificate");

    let cert = directory.path().join("cert.pem");
    let key = directory.path().join("key.pem");
    let client_ca = directory.path().join("client-ca.pem");
    let client_cert_path = directory.path().join("client-cert.pem");
    let client_key_path = directory.path().join("client-key.pem");
    std::fs::write(&cert, server_cert.pem()).expect("write the test certificate");
    std::fs::write(&key, server_key.serialize_pem()).expect("write the test key");
    std::fs::write(&client_ca, ca_cert.pem()).expect("write the client CA");
    std::fs::write(&client_cert_path, client_cert.pem()).expect("write the client certificate");
    std::fs::write(&client_key_path, client_key.serialize_pem()).expect("write the client key");

    TlsMaterial {
        _directory: directory,
        cert,
        key,
        client_ca,
        client_cert: client_cert_path,
        client_key: client_key_path,
        client_cert_der: client_cert.der().to_vec(),
    }
}

// ---------------------------------------------------------------------------
// HTTP/3 client
// ---------------------------------------------------------------------------

/// Accepts the test's self-signed certificate while still verifying handshake
/// signatures through the ring provider (no `unsafe`, no new dev dependency).
#[derive(Debug)]
struct AcceptAnyServerCertificate {
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for AcceptAnyServerCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// One fully read HTTP/3 response.
#[derive(Debug)]
struct CapturedResponse {
    status: u16,
    headers: http::HeaderMap,
    body: Vec<u8>,
}

impl CapturedResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("response body is JSON")
    }
}

/// QUIC varint encoding used by the WebTransport stream header.
fn write_varint(buffer: &mut BytesMut, value: u64) {
    match value {
        0..=0x3f => buffer.put_u8(value as u8),
        0x40..=0x3fff => buffer.put_u16(0x4000 | value as u16),
        0x4000..=0x3fff_ffff => buffer.put_u32(0x8000_0000 | value as u32),
        _ => buffer.put_u64(0xc000_0000_0000_0000 | value),
    }
}

/// A connected h3 client plus its connection driver task.
struct H3Client {
    _endpoint: quinn::Endpoint,
    _connection: quinn::Connection,
    _driver: tokio::task::JoinHandle<()>,
    send_request: h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>,
}

impl H3Client {
    async fn connect(address: SocketAddr) -> Self {
        Self::connect_with_client_auth(address, None).await
    }

    async fn connect_with_client_auth(
        address: SocketAddr,
        client_auth: Option<(&std::path::Path, &std::path::Path)>,
    ) -> Self {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("TLS 1.3 client config")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCertificate { provider }));
        let mut tls = match client_auth {
            Some((cert_path, key_path)) => {
                let mut cert_reader = std::io::BufReader::new(
                    std::fs::File::open(cert_path).expect("open the client certificate"),
                );
                let certificates = CertificateDer::pem_reader_iter(&mut cert_reader)
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .expect("parse the client certificate");
                let mut key_reader = std::io::BufReader::new(
                    std::fs::File::open(key_path).expect("open the client key"),
                );
                let key =
                    PrivateKeyDer::from_pem_reader(&mut key_reader).expect("parse the client key");
                builder
                    .with_client_auth_cert(certificates, key)
                    .expect("client-auth TLS config")
            }
            None => builder.with_no_client_auth(),
        };
        tls.alpn_protocols = vec![b"h3".to_vec()];

        let quic_client_config = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
            .expect("QUIC-compatible client config");
        let mut endpoint = quinn::Endpoint::client(
            "127.0.0.1:0"
                .parse()
                .expect("loopback client socket address"),
        )
        .expect("client QUIC endpoint");
        endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(quic_client_config)));

        let connection = endpoint
            .connect(address, "localhost")
            .expect("client QUIC connect")
            .await
            .expect("QUIC handshake");
        let mut h3_builder = h3::client::builder();
        // RFC 9220 extended CONNECT is required for WebSocket and WebTransport.
        h3_builder
            .enable_extended_connect(true)
            .enable_webtransport(true)
            .enable_datagram(true)
            .webtransport_drafts(h3::ext::WebTransportDraft::ALL.to_vec());
        let (mut h3_connection, send_request) = h3_builder
            .build(h3_quinn::Connection::new(connection.clone()))
            .await
            .expect("HTTP/3 client connection");
        let driver = tokio::spawn(async move {
            let _ = std::future::poll_fn(|cx| h3_connection.poll_close(cx)).await;
        });

        Self {
            _endpoint: endpoint,
            _connection: connection,
            _driver: driver,
            send_request,
        }
    }

    async fn get(&mut self, address: SocketAddr, path: &str) -> CapturedResponse {
        self.request(address, http::Method::GET, path, None).await
    }

    /// Like [`Self::get`], but returns a transport/stream error instead of
    /// panicking (used by the admission-rejection test).
    async fn try_get(
        &mut self,
        address: SocketAddr,
        path: &str,
    ) -> std::result::Result<CapturedResponse, String> {
        let uri = format!("https://localhost:{}{}", address.port(), path);
        let request = http::Request::builder()
            .method(http::Method::GET)
            .uri(uri)
            .body(())
            .expect("build the HTTP/3 request");
        let mut stream = self
            .send_request
            .send_request(request)
            .await
            .map_err(|error| format!("send_request: {error:?}"))?;
        let _ = stream.finish().await;
        let response = stream
            .recv_response()
            .await
            .map_err(|error| format!("recv_response: {error:?}"))?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let mut body = Vec::new();
        while let Some(mut chunk) = stream
            .recv_data()
            .await
            .map_err(|error| format!("recv_data: {error:?}"))?
        {
            let length = chunk.remaining();
            body.extend_from_slice(&chunk.copy_to_bytes(length));
        }
        Ok(CapturedResponse {
            status,
            headers,
            body,
        })
    }

    /// Open a WebSocket over HTTP/3 via RFC 9220 extended CONNECT.
    ///
    /// The h3 request stream is bridged to a duplex stream, then handed to
    /// `tokio-tungstenite` in raw-socket mode: the HTTP/3 extended CONNECT is
    /// the WebSocket handshake transport, so the HTTP/1.1 `101` handshake is
    /// not repeated on the stream.
    async fn websocket(
        &mut self,
        address: SocketAddr,
        path: &str,
    ) -> WebSocketStream<tokio::io::DuplexStream> {
        self.websocket_with(address, path, true).await
    }

    /// RFC 9220-conformant client: extended CONNECT does not use the HTTP/1.1
    /// `Sec-WebSocket-Key`/`Accept` exchange, so compliant h3 clients omit the
    /// key. The adapter must still accept them.
    async fn websocket_without_key(
        &mut self,
        address: SocketAddr,
        path: &str,
    ) -> WebSocketStream<tokio::io::DuplexStream> {
        self.websocket_with(address, path, false).await
    }

    async fn websocket_with(
        &mut self,
        address: SocketAddr,
        path: &str,
        include_key: bool,
    ) -> WebSocketStream<tokio::io::DuplexStream> {
        let uri = format!("https://localhost:{}{}", address.port(), path);
        let mut builder = http::Request::builder()
            .method(http::Method::CONNECT)
            .uri(uri)
            .header("sec-websocket-version", "13");
        if include_key {
            builder = builder.header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
        }
        let mut request = builder
            .body(())
            .expect("build the HTTP/3 extended CONNECT request");
        request
            .extensions_mut()
            .insert(h3::ext::Protocol::WEBSOCKET);

        let mut stream = self
            .send_request
            .send_request(request)
            .await
            .expect("send the extended CONNECT request");
        let response = stream
            .recv_response()
            .await
            .expect("receive the extended CONNECT response");
        assert_eq!(
            response.status(),
            http::StatusCode::OK,
            "extended CONNECT must be accepted with 200"
        );

        let (mut send_half, mut recv_half) = stream.split();
        let (session_io, bridge_io) = tokio::io::duplex(64 * 1024);
        let (mut bridge_read, mut bridge_write) = tokio::io::split(bridge_io);

        // h3 → duplex.
        tokio::spawn(async move {
            while let Ok(Some(mut chunk)) = recv_half.recv_data().await {
                let remaining = chunk.remaining();
                if remaining == 0 {
                    continue;
                }
                let bytes = chunk.copy_to_bytes(remaining);
                if bridge_write.write_all(&bytes).await.is_err() {
                    break;
                }
            }
            let _ = bridge_write.shutdown().await;
        });

        // duplex → h3.
        tokio::spawn(async move {
            let mut buffer = [0u8; 16 * 1024];
            loop {
                match bridge_read.read(&mut buffer).await {
                    Ok(0) => {
                        let _ = send_half.finish().await;
                        break;
                    }
                    Ok(read) => {
                        if send_half
                            .send_data(Bytes::copy_from_slice(&buffer[..read]))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        WebSocketStream::from_raw_socket(session_io, Role::Client, None).await
    }

    /// Open a WebTransport session (RFC 9220 extended CONNECT
    /// `:protocol: webtransport`). Returns the client QUIC connection, the
    /// CONNECT request stream (keep it alive for the session lifetime), and the
    /// WebTransport session id.
    async fn webtransport(
        &mut self,
        address: SocketAddr,
        path: &str,
    ) -> (
        quinn::Connection,
        h3::client::RequestStream<h3_quinn::BidiStream<Bytes>, Bytes>,
        u64,
    ) {
        let uri = format!("https://localhost:{}{}", address.port(), path);
        let mut request = http::Request::builder()
            .method(http::Method::CONNECT)
            .uri(uri)
            .body(())
            .expect("build the WebTransport CONNECT request");
        request
            .extensions_mut()
            .insert(h3::ext::Protocol::WEB_TRANSPORT);

        let mut stream = self
            .send_request
            .send_request(request)
            .await
            .expect("send the WebTransport CONNECT request");
        let response = stream
            .recv_response()
            .await
            .expect("receive the WebTransport CONNECT response");
        assert_eq!(
            response.status(),
            http::StatusCode::OK,
            "WebTransport extended CONNECT must be accepted with 200"
        );
        let session_id = stream.id().into_inner();
        (self._connection.clone(), stream, session_id)
    }

    /// Send an extended CONNECT with the given `:protocol` and return the
    /// complete response (used for WebTransport / negative WS cases).
    async fn extended_connect(
        &mut self,
        address: SocketAddr,
        path: &str,
        protocol: h3::ext::Protocol,
        include_key: bool,
    ) -> CapturedResponse {
        let uri = format!("https://localhost:{}{}", address.port(), path);
        let mut builder = http::Request::builder()
            .method(http::Method::CONNECT)
            .uri(uri)
            .header("sec-websocket-version", "13");
        if include_key {
            builder = builder.header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
        }
        let mut request = builder
            .body(())
            .expect("build the extended CONNECT request");
        request.extensions_mut().insert(protocol);
        self.capture_request(request).await
    }

    /// Send a standard CONNECT (no `:protocol`) and return the response.
    async fn plain_connect(&mut self, address: SocketAddr, path: &str) -> CapturedResponse {
        let uri = format!("https://localhost:{}{}", address.port(), path);
        let request = http::Request::builder()
            .method(http::Method::CONNECT)
            .uri(uri)
            .body(())
            .expect("build the CONNECT request");
        self.capture_request(request).await
    }

    async fn capture_request(&mut self, request: http::Request<()>) -> CapturedResponse {
        let mut stream = self
            .send_request
            .send_request(request)
            .await
            .expect("send the HTTP/3 request");
        let _ = stream.finish().await;
        let response = stream
            .recv_response()
            .await
            .expect("receive the HTTP/3 response headers");
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let mut body = Vec::new();
        while let Some(mut chunk) = stream.recv_data().await.expect("read an HTTP/3 body frame") {
            let length = chunk.remaining();
            body.extend_from_slice(&chunk.copy_to_bytes(length));
        }
        CapturedResponse {
            status,
            headers,
            body,
        }
    }

    async fn post_json(
        &mut self,
        address: SocketAddr,
        path: &str,
        body: &'static str,
    ) -> CapturedResponse {
        self.request(
            address,
            http::Method::POST,
            path,
            Some(("application/json", body)),
        )
        .await
    }

    /// Send one request and read the complete response.
    ///
    /// Only the response is authoritative here: `send_data`/`finish` errors are
    /// ignored because the server may legitimately stop reading an oversized
    /// request body before the client has finished sending it.
    async fn request(
        &mut self,
        address: SocketAddr,
        method: http::Method,
        path: &str,
        body: Option<(&'static str, &'static str)>,
    ) -> CapturedResponse {
        let uri = format!("https://localhost:{}{}", address.port(), path);
        let mut builder = http::Request::builder().method(method).uri(uri);
        if let Some((content_type, body)) = body {
            builder = builder
                .header("content-type", content_type)
                .header("content-length", body.len());
        }
        let request = builder.body(()).expect("build the HTTP/3 request");

        let mut stream = self
            .send_request
            .send_request(request)
            .await
            .expect("send the HTTP/3 request");
        if let Some((_, body)) = body {
            let _ = stream.send_data(Bytes::from_static(body.as_bytes())).await;
        }
        let _ = stream.finish().await;

        let response = stream
            .recv_response()
            .await
            .expect("receive the HTTP/3 response headers");
        let status = response.status().as_u16();
        let headers = response.headers().clone();

        let mut response_body = Vec::new();
        while let Some(mut chunk) = stream.recv_data().await.expect("read an HTTP/3 body frame") {
            let length = chunk.remaining();
            response_body.extend_from_slice(&chunk.copy_to_bytes(length));
        }
        CapturedResponse {
            status,
            headers,
            body: response_body,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn h3_transport_serves_the_production_pipeline_over_quic() {
    let tls = self_signed_tls();
    let transport = H3Transport::new(H3Config::new("127.0.0.1:0", tls.cert, tls.key))
        .expect("build the HTTP/3 transport");
    let address = transport.local_addr();

    let host = TestHost::from_builder(Host::builder().mode(AppMode::Development).no_spa());
    let engine_shutdown = host.engine_handle();
    let transport: Arc<dyn IHttpTransport> = Arc::new(transport);
    // Same shape as the host: the transport backend runs as a spawned task and
    // is stopped through the engine shutdown handle.
    let server_task = tokio::spawn(async move { host.run_http_transport(transport).await });

    tokio::time::timeout(TEST_TIMEOUT, async move {
        let mut client = H3Client::connect(address).await;

        // 1. GET -> JSON handler.
        let hello = client.get(address, "/h3/hello").await;
        assert_eq!(hello.status, 200);
        assert_eq!(hello.header("content-type"), Some("application/json"));
        assert_eq!(hello.json()["message"], "hello h3");

        // 2. POST JSON -> request binding + handler dispatch.
        let echo = client
            .post_json(address, "/h3/echo", r#"{"data":"echo"}"#)
            .await;
        assert_eq!(echo.status, 200);
        assert_eq!(
            serde_json::from_slice::<String>(&echo.body).expect("echo body is a JSON string"),
            "echo"
        );

        // 3. Unknown route -> the engine's 404 problem+json writer.
        let missing = client.get(address, "/h3/missing").await;
        assert_eq!(missing.status, 404);
        assert!(
            missing
                .header("content-type")
                .expect("404 content type")
                .starts_with("application/problem+json"),
            "404 must be problem+json"
        );
        let missing = missing.json();
        assert_eq!(missing["status"], 404);
        assert_eq!(missing["code"], "not_found");

        // 4. Graceful shutdown: the transport task must end in bounded time and
        // must not panic (a panic would surface as a `JoinError`).
        engine_shutdown.shutdown();
        let transport_result = server_task
            .await
            .expect("the HTTP/3 transport task did not panic");
        transport_result.expect("the HTTP/3 transport returned an error");
    })
    .await
    .expect("HTTP/3 e2e test finished within the timeout");
}

#[tokio::test]
async fn h3_connection_without_a_first_request_is_closed_by_the_deadline() {
    let tls = self_signed_tls();
    let transport = H3Transport::new(H3Config::new("127.0.0.1:0", tls.cert, tls.key))
        .expect("build the HTTP/3 transport");
    let address = transport.local_addr();

    let host = TestHost::from_builder(
        Host::builder()
            .mode(AppMode::Development)
            .no_spa()
            .configure(|app| {
                app.options(|options| options.server.first_request_timeout_seconds = 1)
            }),
    );
    let engine_shutdown = host.engine_handle();
    let transport: Arc<dyn IHttpTransport> = Arc::new(transport);
    let server_task = tokio::spawn(async move { host.run_http_transport(transport).await });

    tokio::time::timeout(TEST_TIMEOUT, async move {
        let client = H3Client::connect(address).await;
        // The client completes the QUIC handshake but never sends a request: the
        // adapter must close the connection after the engine's first-request
        // deadline instead of pinning an admission slot indefinitely.
        tokio::time::timeout(Duration::from_secs(5), client._connection.closed())
            .await
            .expect("the transport must close a connection without a first request");

        engine_shutdown.shutdown();
        server_task
            .await
            .expect("the HTTP/3 transport task did not panic")
            .expect("the HTTP/3 transport returned an error");
    })
    .await
    .expect("HTTP/3 first-request-deadline test finished within the timeout");
}

#[tokio::test]
async fn h3_transport_rejects_a_request_body_above_the_cap_with_413() {
    let tls = self_signed_tls();
    let transport = H3Transport::new(
        H3Config::new("127.0.0.1:0", tls.cert, tls.key).with_max_request_body_bytes(8),
    )
    .expect("build the HTTP/3 transport");
    let address = transport.local_addr();

    let host = TestHost::from_builder(Host::builder().mode(AppMode::Development).no_spa());
    let engine_shutdown = host.engine_handle();
    let transport: Arc<dyn IHttpTransport> = Arc::new(transport);
    let server_task = tokio::spawn(async move { host.run_http_transport(transport).await });

    tokio::time::timeout(TEST_TIMEOUT, async move {
        let mut client = H3Client::connect(address).await;
        // 14 bytes: over the 8-byte cap, small enough to be sent in one chunk.
        let rejected = client
            .post_json(address, "/h3/echo", r#"{"data":"echo"}"#)
            .await;
        assert_eq!(rejected.status, 413);
        assert!(
            rejected
                .header("content-type")
                .expect("413 content type")
                .starts_with("application/problem+json"),
            "413 must be problem+json"
        );
        let problem = rejected.json();
        assert_eq!(problem["status"], 413);
        assert_eq!(problem["code"], "payload_too_large");

        // A rejected request stream must not take the connection down: the next
        // request on the same QUIC connection is served normally.
        let after = client.get(address, "/h3/hello").await;
        assert_eq!(after.status, 200);
        assert_eq!(after.json()["message"], "hello h3");

        engine_shutdown.shutdown();
        let transport_result = server_task
            .await
            .expect("the HTTP/3 transport task did not panic");
        transport_result.expect("the HTTP/3 transport returned an error");
    })
    .await
    .expect("HTTP/3 413 test finished within the timeout");
}

#[tokio::test]
async fn h3_transport_streams_an_sse_response_frame_by_frame() {
    let tls = self_signed_tls();
    let transport = H3Transport::new(H3Config::new("127.0.0.1:0", tls.cert, tls.key))
        .expect("build the HTTP/3 transport");
    let address = transport.local_addr();

    let host = TestHost::from_builder(Host::builder().mode(AppMode::Development).no_spa());
    let engine_shutdown = host.engine_handle();
    let transport: Arc<dyn IHttpTransport> = Arc::new(transport);
    let server_task = tokio::spawn(async move { host.run_http_transport(transport).await });

    tokio::time::timeout(TEST_TIMEOUT, async move {
        let mut client = H3Client::connect(address).await;
        let response = client.get(address, "/h3/sse").await;
        assert_eq!(response.status, 200);
        assert!(
            response
                .header("content-type")
                .expect("SSE content type")
                .starts_with("text/event-stream"),
            "SSE response must keep its content type"
        );
        let body = String::from_utf8_lossy(&response.body);
        assert!(
            body.contains("data:first"),
            "missing first SSE frame: {body:?}"
        );
        assert!(
            body.contains("data:second"),
            "missing second SSE frame: {body:?}"
        );

        engine_shutdown.shutdown();
        let transport_result = server_task
            .await
            .expect("the HTTP/3 transport task did not panic");
        transport_result.expect("the HTTP/3 transport returned an error");
    })
    .await
    .expect("HTTP/3 SSE test finished within the timeout");
}

#[tokio::test]
async fn h3_mtls_verifies_the_client_certificate_and_injects_its_fingerprint() {
    let tls = self_signed_tls();
    let transport = H3Transport::new(
        H3Config::new("127.0.0.1:0", tls.cert, tls.key)
            .with_client_auth(&tls.client_ca, TlsClientAuth::Required),
    )
    .expect("build the HTTP/3 transport with mTLS");
    let address = transport.local_addr();

    let host = TestHost::from_builder(Host::builder().mode(AppMode::Development).no_spa());
    let engine_shutdown = host.engine_handle();
    let transport: Arc<dyn IHttpTransport> = Arc::new(transport);
    let server_task = tokio::spawn(async move { host.run_http_transport(transport).await });

    tokio::time::timeout(TEST_TIMEOUT, async move {
        let mut client =
            H3Client::connect_with_client_auth(address, Some((&tls.client_cert, &tls.client_key)))
                .await;
        let response = client.get(address, "/h3/cert").await;
        assert_eq!(response.status, 200);

        let digest = Sha256::digest(&tls.client_cert_der);
        let expected: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        let header = serde_json::from_slice::<String>(&response.body)
            .expect("fingerprint route returns a JSON string");
        assert_eq!(
            header, expected,
            "the engine must receive the SHA-256 of the verified client leaf"
        );

        engine_shutdown.shutdown();
        let transport_result = server_task
            .await
            .expect("the HTTP/3 transport task did not panic");
        transport_result.expect("the HTTP/3 transport returned an error");
    })
    .await
    .expect("HTTP/3 mTLS test finished within the timeout");
}

#[tokio::test]
async fn h3_goaway_drain_finishes_in_flight_requests() {
    let tls = self_signed_tls();
    let transport = H3Transport::new(H3Config::new("127.0.0.1:0", tls.cert, tls.key))
        .expect("build the HTTP/3 transport");
    let address = transport.local_addr();

    let host = TestHost::from_builder(Host::builder().mode(AppMode::Development).no_spa());
    let engine_shutdown = host.engine_handle();
    let transport: Arc<dyn IHttpTransport> = Arc::new(transport);
    let server_task = tokio::spawn(async move { host.run_http_transport(transport).await });

    tokio::time::timeout(TEST_TIMEOUT, async move {
        let mut client = H3Client::connect(address).await;

        // The handler sleeps longer than the shutdown delay: the per-connection
        // GOAWAY drain must let this request finish instead of resetting it.
        let request = client.get(address, "/h3/slow");
        let stop = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            engine_shutdown.shutdown();
        };
        let (response, ()) = tokio::join!(request, stop);
        assert_eq!(response.status, 200, "in-flight request was reset");
        assert_eq!(
            serde_json::from_slice::<String>(&response.body).expect("slow handler JSON"),
            "slow done"
        );

        let transport_result = server_task
            .await
            .expect("the HTTP/3 transport task did not panic");
        transport_result.expect("the HTTP/3 transport returned an error");
    })
    .await
    .expect("HTTP/3 GOAWAY drain test finished within the timeout");
}

#[tokio::test]
async fn h3_connections_share_the_engine_admission_governor() {
    let tls = self_signed_tls();
    let transport = H3Transport::new(H3Config::new("127.0.0.1:0", tls.cert, tls.key))
        .expect("build the HTTP/3 transport");
    let address = transport.local_addr();

    let builder = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.options(|options| {
                options.app.max_connections = 1;
                options.server.admission_timeout_ms = 100;
            })
        });
    let host = TestHost::from_builder(builder);
    let engine_shutdown = host.engine_handle();
    let transport: Arc<dyn IHttpTransport> = Arc::new(transport);
    let server_task = tokio::spawn(async move { host.run_http_transport(transport).await });

    tokio::time::timeout(TEST_TIMEOUT, async move {
        // First QUIC connection holds the single admission slot.
        let mut first = H3Client::connect(address).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let hello = first.get(address, "/h3/hello").await;
        assert_eq!(hello.status, 200);

        // A second connection completes the QUIC handshake but the engine
        // admission times out after 100ms and the transport closes it; the
        // request must fail instead of being served past `MaxConnections=1`.
        let mut second = H3Client::connect(address).await;
        let outcome =
            tokio::time::timeout(Duration::from_secs(3), second.try_get(address, "/h3/hello"))
                .await
                .expect("the rejected request finished");
        assert!(
            outcome.is_err(),
            "over-capacity connection was served: {outcome:?}"
        );

        drop(first);
        drop(second);
        engine_shutdown.shutdown();
        let transport_result = server_task
            .await
            .expect("the HTTP/3 transport task did not panic");
        transport_result.expect("the HTTP/3 transport returned an error");
    })
    .await
    .expect("HTTP/3 admission test finished within the timeout");
}

#[tokio::test]
async fn h3_websocket_echoes_text_and_binary_over_extended_connect() {
    let tls = self_signed_tls();
    let transport = H3Transport::new(H3Config::new("127.0.0.1:0", tls.cert, tls.key))
        .expect("build the HTTP/3 transport");
    let address = transport.local_addr();

    let host = TestHost::from_builder(
        Host::builder()
            .mode(AppMode::Development)
            .no_spa()
            .use_websocket("/h3/ws", Arc::new(H3WebSocketEcho)),
    );
    let engine_shutdown = host.engine_handle();
    let transport: Arc<dyn IHttpTransport> = Arc::new(transport);
    let server_task = tokio::spawn(async move { host.run_http_transport(transport).await });

    tokio::time::timeout(TEST_TIMEOUT, async move {
        let mut client = H3Client::connect(address).await;
        // RFC 9220 extended CONNECT does not use Sec-WebSocket-Key, so test
        // both the legacy key-bearing form and a conformant key-less client.
        for include_key in [true, false] {
            let mut socket = if include_key {
                client.websocket(address, "/h3/ws").await
            } else {
                client.websocket_without_key(address, "/h3/ws").await
            };

            socket
                .send(Message::Text("hello websocket h3".into()))
                .await
                .expect("send a text frame over extended CONNECT");
            match socket.next().await.expect("text echo frame") {
                Ok(Message::Text(text)) => assert_eq!(text, "hello websocket h3"),
                other => panic!("expected a text echo, got {other:?}"),
            }

            socket
                .send(Message::Binary(vec![1, 2, 3, 4].into()))
                .await
                .expect("send a binary frame over extended CONNECT");
            match socket.next().await.expect("binary echo frame") {
                Ok(Message::Binary(bytes)) => assert_eq!(bytes.as_ref(), &[1, 2, 3, 4]),
                other => panic!("expected a binary echo, got {other:?}"),
            }

            // Complete the WebSocket close handshake so the engine session ends
            // as `ClientClosed` rather than as a transport reset.
            socket
                .close(None)
                .await
                .expect("send WebSocket close frame");
            let _ = socket.next().await;
        }
        engine_shutdown.shutdown();
        let transport_result = server_task
            .await
            .expect("the HTTP/3 transport task did not panic");
        transport_result.expect("the HTTP/3 transport returned an error");
    })
    .await
    .expect("HTTP/3 WebSocket test finished within the timeout");
}

#[tokio::test]
async fn h3_extended_connect_rejects_invalid_tunnels() {
    let tls = self_signed_tls();
    let transport = H3Transport::new(H3Config::new("127.0.0.1:0", tls.cert, tls.key))
        .expect("build the HTTP/3 transport");
    let address = transport.local_addr();

    let host = TestHost::from_builder(
        Host::builder()
            .mode(AppMode::Development)
            .no_spa()
            .use_websocket("/h3/ws", Arc::new(H3WebSocketEcho)),
    );
    let engine_shutdown = host.engine_handle();
    let transport: Arc<dyn IHttpTransport> = Arc::new(transport);
    let server_task = tokio::spawn(async move { host.run_http_transport(transport).await });

    tokio::time::timeout(TEST_TIMEOUT, async move {
        let mut client = H3Client::connect(address).await;

        // A websocket extended CONNECT to a normal GET route must never become
        // a successful tunnel carrying the handler's normal 2xx body.
        let not_websocket = client
            .extended_connect(address, "/h3/hello", h3::ext::Protocol::WEBSOCKET, false)
            .await;
        assert_eq!(not_websocket.status, 404);
        assert!(
            not_websocket
                .header("content-type")
                .expect("404 content type")
                .starts_with("application/problem+json"),
            "failed extended CONNECT must be problem+json"
        );

        // A standard CONNECT without `:protocol` is not an HTTP request.
        let plain = client.plain_connect(address, "/h3/hello").await;
        assert_eq!(plain.status, 400);
        assert_eq!(plain.json()["code"], "bad_request");

        // An unknown/unsupported extended-CONNECT protocol is rejected.
        let webtransport_without_handler = client
            .extended_connect(address, "/h3/wt", h3::ext::Protocol::WEB_TRANSPORT, false)
            .await;
        assert_eq!(webtransport_without_handler.status, 501);
        assert_eq!(
            webtransport_without_handler.json()["code"],
            "not_implemented"
        );

        engine_shutdown.shutdown();
        let transport_result = server_task
            .await
            .expect("the HTTP/3 transport task did not panic");
        transport_result.expect("the HTTP/3 transport returned an error");
    })
    .await
    .expect("HTTP/3 extended CONNECT rejection test finished within the timeout");
}

#[tokio::test]
async fn h3_transport_keeps_idle_connections_alive_with_keep_alive() {
    let tls = self_signed_tls();
    let transport = H3Transport::new(
        H3Config::new("127.0.0.1:0", tls.cert, tls.key)
            .with_keep_alive_interval(Some(Duration::from_secs(1)))
            .with_max_idle_timeout(Some(Duration::from_secs(2))),
    )
    .expect("build the HTTP/3 transport");
    let address = transport.local_addr();

    let host = TestHost::from_builder(Host::builder().mode(AppMode::Development).no_spa());
    let engine_shutdown = host.engine_handle();
    let transport: Arc<dyn IHttpTransport> = Arc::new(transport);
    let server_task = tokio::spawn(async move { host.run_http_transport(transport).await });

    tokio::time::timeout(TEST_TIMEOUT, async move {
        let mut client = H3Client::connect(address).await;
        // Longer than the configured idle timeout: without keep-alive this
        // connection would be closed and the request would fail.
        tokio::time::sleep(Duration::from_secs(3)).await;
        let hello = client.get(address, "/h3/hello").await;
        assert_eq!(hello.status, 200);

        engine_shutdown.shutdown();
        let transport_result = server_task
            .await
            .expect("the HTTP/3 transport task did not panic");
        transport_result.expect("the HTTP/3 transport returned an error");
    })
    .await
    .expect("HTTP/3 keep-alive test finished within the timeout");
}

#[tokio::test]
async fn h3_webtransport_denies_before_accepting() {
    let tls = self_signed_tls();
    let transport = H3Transport::new(
        H3Config::new("127.0.0.1:0", tls.cert, tls.key)
            .with_webtransport_handler(Arc::new(H3WebTransportDeny)),
    )
    .expect("build the HTTP/3 transport with a denying WebTransport handler");
    let address = transport.local_addr();

    let host = TestHost::from_builder(Host::builder().mode(AppMode::Development).no_spa());
    let engine_shutdown = host.engine_handle();
    let transport: Arc<dyn IHttpTransport> = Arc::new(transport);
    let server_task = tokio::spawn(async move { host.run_http_transport(transport).await });

    tokio::time::timeout(TEST_TIMEOUT, async move {
        let mut client = H3Client::connect(address).await;
        let denied = client
            .extended_connect(
                address,
                "/h3/webtransport",
                h3::ext::Protocol::WEB_TRANSPORT,
                false,
            )
            .await;
        assert_eq!(denied.status, 403);
        assert_eq!(denied.json()["code"], "forbidden");

        engine_shutdown.shutdown();
        let transport_result = server_task
            .await
            .expect("the HTTP/3 transport task did not panic");
        transport_result.expect("the HTTP/3 transport returned an error");
    })
    .await
    .expect("HTTP/3 WebTransport denial test finished within the timeout");
}

#[tokio::test]
async fn h3_webtransport_default_authorize_is_fail_closed() {
    // A handler that does not override `authorize` must not receive an
    // unauthenticated WebTransport tunnel: CONNECT bypasses the HTTP pipeline.
    let tls = self_signed_tls();
    let transport = H3Transport::new(
        H3Config::new("127.0.0.1:0", tls.cert, tls.key)
            .with_webtransport_handler(Arc::new(H3WebTransportNoAuthorize)),
    )
    .expect("build the HTTP/3 transport with a handler that does not authorize");
    let address = transport.local_addr();

    let host = TestHost::from_builder(Host::builder().mode(AppMode::Development).no_spa());
    let engine_shutdown = host.engine_handle();
    let transport: Arc<dyn IHttpTransport> = Arc::new(transport);
    let server_task = tokio::spawn(async move { host.run_http_transport(transport).await });

    tokio::time::timeout(TEST_TIMEOUT, async move {
        let mut client = H3Client::connect(address).await;
        let denied = client
            .extended_connect(
                address,
                "/h3/webtransport",
                h3::ext::Protocol::WEB_TRANSPORT,
                false,
            )
            .await;
        assert_eq!(denied.status, 403);
        assert_eq!(denied.json()["code"], "forbidden");

        engine_shutdown.shutdown();
        let transport_result = server_task
            .await
            .expect("the HTTP/3 transport task did not panic");
        transport_result.expect("the HTTP/3 transport returned an error");
    })
    .await
    .expect("HTTP/3 default WebTransport authorization test finished within the timeout");
}

#[tokio::test]
async fn h3_webtransport_bidi_stream_echoes_over_extended_connect() {
    let tls = self_signed_tls();
    let transport = H3Transport::new(
        H3Config::new("127.0.0.1:0", tls.cert, tls.key)
            .with_webtransport_handler(Arc::new(H3WebTransportEcho)),
    )
    .expect("build the HTTP/3 transport with WebTransport");
    let address = transport.local_addr();

    let host = TestHost::from_builder(Host::builder().mode(AppMode::Development).no_spa());
    let engine_shutdown = host.engine_handle();
    let transport: Arc<dyn IHttpTransport> = Arc::new(transport);
    let server_task = tokio::spawn(async move { host.run_http_transport(transport).await });

    tokio::time::timeout(TEST_TIMEOUT, async move {
        let mut client = H3Client::connect(address).await;
        let (quic, connect_stream, session_id) =
            client.webtransport(address, "/h3/webtransport").await;

        let (mut send, mut recv) = quic
            .open_bi()
            .await
            .expect("open a WebTransport bidi stream");
        let mut header = BytesMut::new();
        write_varint(&mut header, 0x41); // WebTransport bidirectional stream type
        write_varint(&mut header, session_id);
        send.write_all(&header)
            .await
            .expect("write the WebTransport stream header");
        send.write_all(b"hello webtransport")
            .await
            .expect("write the WebTransport payload");
        send.finish().expect("finish the WebTransport stream");

        let echoed = recv
            .read_to_end(64 * 1024)
            .await
            .expect("read the WebTransport echo");
        assert_eq!(echoed, b"hello webtransport");

        drop(connect_stream);
        drop(client);
        engine_shutdown.shutdown();
        let transport_result = server_task
            .await
            .expect("the HTTP/3 transport task did not panic");
        transport_result.expect("the HTTP/3 transport returned an error");
    })
    .await
    .expect("HTTP/3 WebTransport test finished within the timeout");
}

#[tokio::test]
async fn h3_transport_returns_promptly_when_shutdown_was_already_requested() {
    let tls = self_signed_tls();
    let transport = H3Transport::new(H3Config::new("127.0.0.1:0", tls.cert, tls.key))
        .expect("build the HTTP/3 transport");

    let host = TestHost::from_builder(Host::builder().mode(AppMode::Development).no_spa());
    let engine_shutdown = host.engine_handle();
    let transport: Arc<dyn IHttpTransport> = Arc::new(transport);

    // Already-draining engine: the backend must close the endpoint and return
    // Ok instead of waiting for a shutdown notification that will never come.
    engine_shutdown.shutdown();
    let result = tokio::time::timeout(Duration::from_secs(5), host.run_http_transport(transport))
        .await
        .expect("transport stopped within the timeout");
    result.expect("the HTTP/3 transport returned an error");
}
