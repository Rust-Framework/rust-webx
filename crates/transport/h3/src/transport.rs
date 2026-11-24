//! The HTTP/3 listener backend: QUIC endpoint, h3 connection loop and the
//! bridge from h3 request streams to the engine's neutral request port.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, Bytes};
use futures_util::StreamExt;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use webx_core::error::{Error, Result};
use webx_engine::transport::{
    IHttpTransport, TransportRequest, TransportResponse, TransportResponseBody, TransportRuntime,
    TransportUpgrade, TransportUpgradeError, UpgradedTransport,
};

use crate::config::H3Config;
use crate::tls;

/// Upper bound on how long graceful shutdown waits for QUIC connections to
/// drain after the endpoint is closed. The engine's own drain budget is longer;
/// a stuck UDP driver must not keep the transport task alive forever.
const SHUTDOWN_WAIT_IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// Upper bound on how long one connection is allowed to keep in-flight request
/// tasks alive after a GOAWAY during graceful shutdown.
const CONNECTION_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Upper bound on how long a connection waits for active WebSocket bridge
/// tasks (close-frame write) before dropping the HTTP/3 connection.
const WEBSOCKET_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// The host aborts a transport backend after this budget; the drain budgets
/// above plus the bounded endpoint shutdown must fit inside it with margin.
const TRANSPORT_STOP_BUDGET: Duration = Duration::from_secs(60);

/// Per-request deadlines mirrored from the engine configuration.
///
/// The engine applies these on the hyper path; the h3 backend reads them from
/// the shared engine state so both transports fail closed on a slow client.
#[derive(Clone, Copy)]
struct RequestTimeouts {
    /// A freshly established QUIC connection must produce its first request
    /// within this deadline (`Server.FirstRequestTimeoutSeconds`).
    first_request: Duration,
    /// Optional deadline for reading the request body and dispatching it
    /// (`Server.RequestTimeoutSeconds`; `None` disables the deadline).
    request: Option<Duration>,
}

/// Maximum request-header section accepted from a client (the h3 default is
/// 16 KiB; HTTP/3 clients can legitimately send larger cookie/authorization
/// headers, and the engine applies its own limits afterwards).
const MAX_FIELD_SECTION_SIZE: u64 = 64 * 1024;

/// `H3_NO_ERROR` (RFC 9114 §8.1): the connection close code used for graceful
/// shutdown.
const H3_NO_ERROR: u32 = 0;

/// Minimal `application/problem+json` body for a request rejected before the
/// engine sees it. The engine's own problem writer is used for every request
/// that is dispatched; this one only covers transport-level rejection.
const PAYLOAD_TOO_LARGE_BODY: &[u8] =
    br#"{"status":413,"code":"payload_too_large","title":"Payload Too Large"}"#;
const BAD_REQUEST_BODY: &[u8] = br#"{"status":400,"code":"bad_request","title":"Bad Request"}"#;
const NOT_FOUND_BODY: &[u8] = br#"{"status":404,"code":"not_found","title":"Not Found"}"#;
const NOT_IMPLEMENTED_BODY: &[u8] =
    br#"{"status":501,"code":"not_implemented","title":"Not Implemented"}"#;
const FORBIDDEN_BODY: &[u8] = br#"{"status":403,"code":"forbidden","title":"Forbidden"}"#;
const GATEWAY_TIMEOUT_BODY: &[u8] =
    br#"{"status":504,"code":"gateway_timeout","title":"Gateway Timeout"}"#;

/// Placeholder key used only to satisfy the existing host WebSocket middleware
/// for RFC 9220 extended CONNECT. HTTP/1.1 key/accept validation does not apply
/// to extended CONNECT (RFC 8441 §5, reused by RFC 9220 §3).
const EXTENDED_CONNECT_WEBSOCKET_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

/// Concrete request-stream type produced by `h3` over a `quinn` connection.
type H3RequestStream = h3::server::RequestStream<h3_quinn::BidiStream<Bytes>, Bytes>;

/// Optional HTTP/3 listener transport for rust-webx.
///
/// Construction binds the UDP socket, loads the TLS material and builds the
/// QUIC server configuration, so configuration errors surface at startup
/// instead of on the first connection. The transport then serves every accepted
/// QUIC connection through the engine's production request pipeline via
/// [`webx_engine::Server::serve_transport_request`].
///
/// See the [crate documentation](crate) for the supported scope and known
/// limitations.
pub struct H3Transport {
    endpoint: quinn::Endpoint,
    local_addr: SocketAddr,
    max_request_body_bytes: usize,
    webtransport_handler: Option<Arc<dyn crate::webtransport::IWebTransportHandler>>,
}

impl std::fmt::Debug for H3Transport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("H3Transport")
            .field("local_addr", &self.local_addr)
            .field("max_request_body_bytes", &self.max_request_body_bytes)
            .finish_non_exhaustive()
    }
}

impl H3Transport {
    /// Bind the QUIC endpoint and prepare the TLS 1.3 / HTTP/3 handshake.
    ///
    /// Startup work, in order:
    ///
    /// 1. parse `config.bind` as an `IP:port` socket address (port `0` is
    ///    supported and resolved by the OS);
    /// 2. load the PEM certificate chain and private key and build a TLS
    ///    1.3-only [`rustls::ServerConfig`] with ALPN `h3`;
    /// 3. convert it to a QUIC server configuration and bind
    ///    [`quinn::Endpoint::server`].
    ///
    /// All failures are returned as readable [`webx_core::error::Error::Http`]
    /// values naming the address or PEM file the operator has to fix; the key
    /// material itself is never part of an error message or log record.
    ///
    /// Must be called from inside a Tokio runtime: quinn spawns the endpoint's
    /// UDP driver on the current runtime, so constructing the transport outside
    /// a runtime context fails with a bind/`no async runtime found` error.
    pub fn new(config: H3Config) -> Result<Self> {
        let bind: SocketAddr = config.bind.parse().map_err(|_| {
            Error::Http(format!(
                "HTTP/3 bind address '{}' is not an IP:port socket address",
                config.bind
            ))
        })?;

        let tls_config = tls::build_server_config(
            &config.cert_path,
            &config.key_path,
            &config.client_ca_path,
            config.client_auth,
        )?;
        let quic_crypto =
            quinn::crypto::rustls::QuicServerConfig::try_from(tls_config).map_err(|err| {
                Error::Http(format!(
                    "HTTP/3 TLS configuration is not QUIC-compatible: {err}"
                ))
            })?;
        let mut quic_transport = quinn::TransportConfig::default();
        quic_transport.keep_alive_interval(config.keep_alive_interval);
        if let Some(timeout) = config.max_idle_timeout {
            let idle_timeout = quinn::IdleTimeout::try_from(timeout).map_err(|err| {
                Error::Http(format!(
                    "HTTP/3 idle timeout {timeout:?} is not a valid QUIC timeout: {err}"
                ))
            })?;
            quic_transport.max_idle_timeout(Some(idle_timeout));
        }
        quic_transport
            .max_concurrent_bidi_streams(config.max_concurrent_bidi_streams.into())
            .max_concurrent_uni_streams(config.max_concurrent_uni_streams.into());

        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_crypto));
        server_config.transport_config(Arc::new(quic_transport));

        let endpoint = quinn::Endpoint::server(server_config, bind)
            .map_err(|err| Error::Http(format!("cannot bind HTTP/3 endpoint to {bind}: {err}")))?;
        let local_addr = endpoint.local_addr().map_err(|err| {
            Error::Http(format!("cannot read the HTTP/3 endpoint address: {err}"))
        })?;

        Ok(Self {
            endpoint,
            local_addr,
            max_request_body_bytes: config.max_request_body_bytes,
            webtransport_handler: config.webtransport_handler,
        })
    }

    /// The bound UDP address (the OS-assigned port when `bind` used port `0`).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Close the QUIC endpoint and wait (bounded) for its driver to become idle.
    async fn shutdown_gracefully(&self) {
        self.endpoint
            .close(quinn::VarInt::from_u32(H3_NO_ERROR), b"shutdown");
        if tokio::time::timeout(SHUTDOWN_WAIT_IDLE_TIMEOUT, self.endpoint.wait_idle())
            .await
            .is_err()
        {
            tracing::debug!(
                timeout = ?SHUTDOWN_WAIT_IDLE_TIMEOUT,
                "h3: QUIC endpoint drain timed out; stopping anyway"
            );
        }
    }
}

#[async_trait::async_trait]
impl IHttpTransport for H3Transport {
    fn kind(&self) -> &'static str {
        "h3"
    }

    fn stop_budget(&self) -> Duration {
        TRANSPORT_STOP_BUDGET
    }

    async fn run(self: Arc<Self>, runtime: TransportRuntime) -> Result<()> {
        tracing::info!(bind = %self.local_addr, "h3: transport started");
        let mut shutdown = runtime.subscribe_shutdown();
        if *shutdown.borrow() {
            self.shutdown_gracefully().await;
            return Ok(());
        }

        // Mirror the engine's own request deadlines: a QUIC client that never
        // sends a request, or that stalls mid-body, must not pin an admission
        // slot shared with the HTTP/1.1+2 listeners.
        let request_timeouts = RequestTimeouts {
            first_request: runtime.shared().config.first_request_timeout,
            request: runtime.shared().config.request_timeout,
        };

        // One signal fans out to every established h3 connection so each sends
        // its own GOAWAY and finishes its in-flight requests before the endpoint
        // is closed. `endpoint.close()` alone would reset those streams.
        let (drain, _) = tokio::sync::watch::channel(false);
        let transport_metrics = runtime.transport_metrics().cloned();
        let mut connections = JoinSet::new();

        loop {
            tokio::select! {
                biased;
                changed = shutdown.changed() => {
                    match changed {
                        // Engine graceful shutdown, or the engine dropped its
                        // sender: stop accepting, GOAWAY connections, drain.
                        Ok(()) if *shutdown.borrow() => break,
                        Ok(()) => continue,
                        Err(_) => break,
                    }
                }
                incoming = self.endpoint.accept() => {
                    // `None` means the endpoint was closed (shutdown already in
                    // progress); stop the accept loop.
                    let Some(incoming) = incoming else { break };
                    let connection_runtime = runtime.clone();
                    let max_request_body_bytes = self.max_request_body_bytes;
                    let webtransport_handler = self.webtransport_handler.clone();
                    let drain = drain.subscribe();
                    let metrics = transport_metrics.clone();
                    connections.spawn(async move {
                        match incoming.await {
                            Ok(connection) => {
                                serve_connection(
                                    connection,
                                    connection_runtime,
                                    max_request_body_bytes,
                                    drain,
                                    metrics,
                                    webtransport_handler,
                                    request_timeouts,
                                )
                                .await;
                            }
                            Err(error) => {
                                if let Some(metrics) = &metrics {
                                    metrics.record_handshake_failure();
                                }
                                tracing::debug!(error = %error, "h3: QUIC handshake failed");
                            }
                        }
                    });
                    // JoinSet retains finished tasks until joined; reap them so
                    // a long-lived endpoint does not leak one entry per closed
                    // QUIC connection.
                    while let Some(result) = connections.try_join_next() {
                        if let Err(error) = result {
                            tracing::debug!(%error, "h3: connection task ended abnormally");
                        }
                    }
                }
            }
        }

        // Tell every connection to GOAWAY and wait (bounded) for its request
        // tasks. A connection that does not finish in time is aborted; the
        // endpoint close below then resets its remaining streams.
        drain.send_replace(true);
        let drain_deadline =
            tokio::time::Instant::now() + CONNECTION_DRAIN_TIMEOUT + SHUTDOWN_WAIT_IDLE_TIMEOUT;
        loop {
            let remaining = drain_deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                tracing::debug!("h3: connection drain timed out; aborting remaining connections");
                connections.abort_all();
                break;
            }
            match tokio::time::timeout(remaining, connections.join_next()).await {
                Ok(Some(Ok(()))) => {}
                Ok(Some(Err(error))) => {
                    tracing::debug!(%error, "h3: connection task ended abnormally");
                }
                Ok(None) => break,
                Err(_) => {
                    tracing::debug!(
                        "h3: connection drain timed out; aborting remaining connections"
                    );
                    connections.abort_all();
                    break;
                }
            }
        }

        self.shutdown_gracefully().await;
        tracing::info!(bind = %self.local_addr, "h3: transport stopped");
        Ok(())
    }
}

/// Serve one established QUIC connection: build its `h3` connection and accept
/// request streams until the peer closes the connection or the engine drains.
///
/// Requests are multiplexed: each accepted request stream is handled in its own
/// task so a large/slow request cannot block other streams on the same QUIC
/// connection, and the accept loop keeps processing control frames (GOAWAY,
/// settings) continuously. `draining` flips to `true` on graceful shutdown: the
/// accept loop then sends a GOAWAY and waits for in-flight request tasks.
///
/// A WebTransport extended CONNECT consumes the HTTP/3 connection (one session
/// per connection); the session handler then owns all remaining streams and
/// datagrams until it ends.
#[allow(clippy::too_many_arguments)]
async fn serve_connection(
    connection: quinn::Connection,
    runtime: TransportRuntime,
    max_request_body_bytes: usize,
    mut draining: tokio::sync::watch::Receiver<bool>,
    transport_metrics: Option<Arc<webx_engine::TransportMetrics>>,
    webtransport_handler: Option<Arc<dyn crate::webtransport::IWebTransportHandler>>,
    request_timeouts: RequestTimeouts,
) {
    let peer = connection.remote_address();
    // One QUIC connection = one `App.MaxConnections` slot, shared with the
    // TCP acceptors. QUIC cannot answer 503 before the handshake, so an
    // over-capacity connection is closed here instead of being served.
    let _admission = match runtime.admit_connection().await {
        Ok(permit) => permit,
        Err(error) => {
            if let Some(metrics) = &transport_metrics {
                metrics.record_rejected();
            }
            tracing::warn!(%peer, %error, "h3: connection rejected by engine admission");
            connection.close(quinn::VarInt::from_u32(0), b"server busy");
            return;
        }
    };
    let _active = transport_metrics
        .as_ref()
        .map(|metrics| metrics.connection_opened());
    let verified_client_cert = client_cert_fingerprint(&connection);
    let server = Arc::new(runtime.server());
    let quic_connection = h3_quinn::Connection::new(connection);

    let mut h3_builder = h3::server::builder();
    h3_builder
        .max_field_section_size(MAX_FIELD_SECTION_SIZE)
        // RFC 9220 extended CONNECT is required for WebSocket over HTTP/3.
        .enable_extended_connect(true);
    if webtransport_handler.is_some() {
        // Only advertise WebTransport when an application handler exists.
        h3_builder
            .enable_webtransport(true)
            .enable_datagram(true)
            .webtransport_drafts(h3::ext::WebTransportDraft::ALL.to_vec());
    }
    let mut h3: Option<h3::server::Connection<h3_quinn::Connection, Bytes>> =
        match h3_builder.build(quic_connection).await {
            Ok(h3) => Some(h3),
            Err(error) => {
                tracing::debug!(%peer, error = %error, "h3: failed to build the HTTP/3 connection");
                return;
            }
        };

    let mut requests = JoinSet::new();
    let websocket_bridges = Arc::new(AtomicUsize::new(0));
    let mut drained_locally = false;
    // A QUIC client that completes the handshake but never sends a request must
    // not keep an admission slot: the engine applies the same first-request
    // deadline on the hyper path.
    let mut first_request_deadline =
        Some(tokio::time::Instant::now() + request_timeouts.first_request);
    loop {
        if *draining.borrow() {
            drained_locally = true;
            break;
        }

        // Poll `accept()` and the drain signal without keeping a mutable borrow
        // of `h3` alive into the match below (WebTransport consumes it).
        let mut accepted = None;
        let mut drain_signal = false;
        let mut first_request_expired = false;
        {
            let Some(conn) = h3.as_mut() else {
                break;
            };
            tokio::select! {
                biased;
                changed = draining.changed() => {
                    drain_signal = changed.is_ok() && *draining.borrow();
                }
                result = async {
                    match first_request_deadline {
                        Some(deadline) => {
                            match tokio::time::timeout_at(deadline, conn.accept()).await {
                                Ok(result) => {
                                    first_request_deadline = None;
                                    Some(result)
                                }
                                Err(_) => None,
                            }
                        }
                        None => Some(conn.accept().await),
                    }
                } => {
                    if let Some(result) = result {
                        accepted = Some(result);
                    } else {
                        first_request_expired = true;
                    }
                }
            }
        }
        if first_request_expired {
            tracing::debug!(
                %peer,
                timeout = ?request_timeouts.first_request,
                "h3: no request within the first-request deadline; closing the connection"
            );
            break;
        }
        if drain_signal {
            drained_locally = true;
            break;
        }
        let Some(accepted) = accepted else { continue };
        let resolver = match accepted {
            Ok(Some(resolver)) => resolver,
            // The peer sent GOAWAY and every accepted request completed.
            Ok(None) => {
                tracing::debug!(%peer, "h3: connection closed by the peer");
                break;
            }
            // A peer-initiated close with H3_NO_ERROR is a normal end.
            Err(error) if error.is_h3_no_error() => {
                tracing::debug!(%peer, "h3: connection finished with H3_NO_ERROR");
                break;
            }
            Err(error) => {
                tracing::debug!(%peer, error = %error, "h3: connection error");
                break;
            }
        };

        let (request, mut stream) = match resolver.resolve_request().await {
            Ok(resolved) => resolved,
            Err(error) => {
                tracing::debug!(%peer, error = %error, "h3: request head rejected");
                continue;
            }
        };

        if crate::webtransport::is_webtransport_request(&request) {
            let Some(handler) = webtransport_handler.clone() else {
                write_response(&mut stream, problem_response(501, NOT_IMPLEMENTED_BODY)).await;
                continue;
            };
            // WebTransport bypasses the HTTP pipeline, so carry the request
            // metadata to the handler for its own authentication/authorization.
            let request_info = crate::webtransport::WebTransportRequestInfo {
                uri: request.uri().clone(),
                method: request.method().clone(),
                headers: request.headers().clone(),
                peer,
                client_cert_fingerprint: verified_client_cert.clone(),
            };
            // Authorize before accepting/committing 200: once the fork sends a
            // 200 there is no way to reject the session with 401/403.
            if let Err(error) = handler.authorize(&request_info).await {
                tracing::debug!(%peer, error = %error, "h3: WebTransport session denied");
                write_response(&mut stream, problem_response(403, FORBIDDEN_BODY)).await;
                continue;
            }
            let Some(conn) = h3.take() else {
                break;
            };
            match h3_webtransport::server::WebTransportSession::accept(
                request,
                stream,
                conn,
                http::HeaderMap::new(),
            )
            .await
            {
                Ok(h3_webtransport::server::Accepted::Session(session)) => {
                    serve_webtransport_session(handler, request_info, session).await;
                    break;
                }
                Ok(h3_webtransport::server::Accepted::NotWebTransport {
                    request,
                    stream,
                    conn,
                }) => {
                    h3 = Some(conn);
                    spawn_request(
                        &mut requests,
                        server.clone(),
                        request,
                        stream,
                        peer,
                        verified_client_cert.clone(),
                        max_request_body_bytes,
                        request_timeouts.request,
                        transport_metrics.clone(),
                        websocket_bridges.clone(),
                    );
                }
                Err(error) => {
                    tracing::debug!(%peer, error = %error, "h3: WebTransport CONNECT failed");
                    break;
                }
            }
            continue;
        }

        spawn_request(
            &mut requests,
            server.clone(),
            request,
            stream,
            peer,
            verified_client_cert.clone(),
            max_request_body_bytes,
            request_timeouts.request,
            transport_metrics.clone(),
            websocket_bridges.clone(),
        );
    }

    if drained_locally {
        if let Some(conn) = h3.as_mut() {
            if let Err(error) = conn.shutdown(0).await {
                tracing::debug!(%peer, error = %error, "h3: failed to send GOAWAY");
            }
        }
    }

    // Let in-flight requests finish (the engine shutdown token ends SSE/streams)
    // before the connection is dropped; bounded so a stuck handler cannot pin the
    // transport forever.
    let requests_drained = async {
        while let Some(result) = requests.join_next().await {
            if let Err(error) = result {
                tracing::warn!(%error, "h3: request task ended abnormally");
            }
        }
    };
    if tokio::time::timeout(CONNECTION_DRAIN_TIMEOUT, requests_drained)
        .await
        .is_err()
    {
        tracing::debug!(%peer, "h3: in-flight requests did not finish; aborting them");
        requests.abort_all();
        while let Some(result) = requests.join_next().await {
            if let Err(error) = result {
                tracing::debug!(%error, "h3: aborted request task");
            }
        }
    }

    // WebSocket bridges outlive their handshake request task: wait (bounded) for
    // the engine session to write its close frame before dropping the HTTP/3
    // connection, matching the hyper path's close-frame behavior.
    let bridge_deadline = tokio::time::Instant::now() + WEBSOCKET_DRAIN_TIMEOUT;
    while websocket_bridges.load(Ordering::SeqCst) > 0
        && tokio::time::Instant::now() < bridge_deadline
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let remaining_bridges = websocket_bridges.load(Ordering::SeqCst);
    if remaining_bridges > 0 {
        tracing::debug!(
            %peer,
            remaining = remaining_bridges,
            "h3: WebSocket bridge drain timed out; dropping the connection"
        );
    }
}

/// Decrements the active WebSocket bridge count when the send pump ends.
struct WebSocketBridgeGuard(Arc<AtomicUsize>);

impl Drop for WebSocketBridgeGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Spawn one ordinary HTTP/3 request task.
#[allow(clippy::too_many_arguments)]
fn spawn_request(
    requests: &mut JoinSet<()>,
    server: Arc<webx_engine::Server>,
    request: http::Request<()>,
    stream: H3RequestStream,
    peer: SocketAddr,
    client_cert_fingerprint: Option<Arc<str>>,
    max_request_body_bytes: usize,
    request_timeout: Option<Duration>,
    metrics: Option<Arc<webx_engine::TransportMetrics>>,
    websocket_bridges: Arc<AtomicUsize>,
) {
    requests.spawn(async move {
        if let Some(metrics) = &metrics {
            metrics.record_request();
        }
        serve_request(
            server,
            request,
            stream,
            peer,
            client_cert_fingerprint,
            max_request_body_bytes,
            request_timeout,
            websocket_bridges,
        )
        .await;
    });
    // `JoinSet` keeps finished tasks (and their output) until they are joined.
    // A long-lived QUIC connection serves an unbounded number of request
    // streams, so finished tasks must be reaped as new ones are accepted or the
    // set grows for the lifetime of the connection.
    while let Some(result) = requests.try_join_next() {
        if let Err(error) = result {
            tracing::warn!(%error, "h3: request task ended abnormally");
        }
    }
}

/// Run the application's WebTransport handler on one accepted session.
async fn serve_webtransport_session(
    handler: Arc<dyn crate::webtransport::IWebTransportHandler>,
    info: crate::webtransport::WebTransportRequestInfo,
    session: crate::webtransport::WebTransportServerSession,
) {
    let session = Arc::new(session);
    if let Err(error) = handler.on_session(info, Arc::clone(&session)).await {
        tracing::warn!(error = %error, "h3: WebTransport handler ended with an error");
    }
    // Give the session a bounded chance to send its close capsule / finish the
    // CONNECT stream instead of dropping the whole QUIC connection abruptly.
    let _ = tokio::time::timeout(Duration::from_secs(1), session.closed()).await;
}

/// Serve one accepted h3 request.
///
/// Plain requests are buffered and dispatched through the engine's neutral
/// request port. RFC 9220 extended CONNECT with `:protocol: websocket` is
/// bridged to the engine's existing `IUpgradeSession` lifecycle: the request is
/// rewritten into the standard WebSocket upgrade form, the engine's host
/// middleware performs the handshake/authorization, and the resulting session
/// gets a duplex byte stream pumped to/from the HTTP/3 request stream.
#[allow(clippy::too_many_arguments)]
async fn serve_request(
    server: Arc<webx_engine::Server>,
    request: http::Request<()>,
    mut stream: H3RequestStream,
    peer: SocketAddr,
    client_cert_fingerprint: Option<Arc<str>>,
    max_request_body_bytes: usize,
    request_timeout: Option<Duration>,
    websocket_bridges: Arc<AtomicUsize>,
) {
    // One deadline covers reading the request body and dispatching it, mirroring
    // the engine's `Server.RequestTimeoutSeconds` on the hyper path.
    let request_deadline = request_timeout.map(|limit| tokio::time::Instant::now() + limit);
    let extended_protocol = request.extensions().get::<h3::ext::Protocol>().copied();
    let websocket = request.method() == http::Method::CONNECT
        && extended_protocol == Some(h3::ext::Protocol::WEBSOCKET);

    let (mut parts, ()) = request.into_parts();
    // A regular HTTP/3 request must not be able to spoof the HTTP/1.1 upgrade
    // headers; only an extended CONNECT with `:protocol: websocket` may enter
    // the WebSocket middleware.
    parts.headers.remove("upgrade");
    parts.headers.remove("connection");
    // HTTP/3 carries the authority in `:authority`, not in a `host` header.
    // Normalize it so application middleware sees the same header as h1/h2.
    if !parts.headers.contains_key(http::header::HOST) {
        if let Some(authority) = parts.uri.authority() {
            if let Ok(value) = http::HeaderValue::from_str(authority.as_str()) {
                parts.headers.insert(http::header::HOST, value);
            }
        }
    }

    // A standard CONNECT without `:protocol` has no tunnel protocol here and
    // must not be routed as an ordinary HTTP request.
    if parts.method == http::Method::CONNECT && extended_protocol.is_none() {
        write_response(&mut stream, problem_response(400, BAD_REQUEST_BODY)).await;
        return;
    }

    if !websocket && extended_protocol.is_some() {
        write_response(&mut stream, problem_response(501, NOT_IMPLEMENTED_BODY)).await;
        return;
    }

    let (body, upgrade) = if websocket {
        // The HTTP/3 stream is the tunnel itself; there is no buffered HTTP body.
        let (io_tx, io_rx) = oneshot::channel::<UpgradedTransport>();
        parts.method = http::Method::GET;
        parts
            .headers
            .insert("upgrade", http::HeaderValue::from_static("websocket"));
        parts
            .headers
            .insert("connection", http::HeaderValue::from_static("upgrade"));
        // RFC 9220 reuses RFC 8441: the HTTP/1.1 `Sec-WebSocket-Key`/`Accept`
        // exchange is not performed for extended CONNECT. Conformant h3
        // clients may omit the key; synthesize the header only to satisfy the
        // existing host middleware, which still computes an Accept value.
        if !parts.headers.contains_key("sec-websocket-key") {
            parts.headers.insert(
                "sec-websocket-key",
                http::HeaderValue::from_static(EXTENDED_CONNECT_WEBSOCKET_KEY),
            );
        }
        let upgrade = TransportUpgrade {
            io: Box::pin(async move {
                io_rx
                    .await
                    .map_err(|error| Box::new(error) as TransportUpgradeError)
            }),
        };
        (Bytes::new(), Some((upgrade, io_tx)))
    } else {
        let body = match read_request_body(&mut stream, max_request_body_bytes, request_deadline)
            .await
        {
            Ok(Some(body)) => body,
            Ok(None) => {
                tracing::debug!(
                    %peer,
                    max = max_request_body_bytes,
                    "h3: request body exceeds the configured cap; answering 413"
                );
                if let Err(error) = respond_payload_too_large(&mut stream).await {
                    tracing::debug!(%peer, error = %error, "h3: failed to write the 413 response");
                }
                return;
            }
            Err(BodyReadError::Timeout) => {
                tracing::debug!(%peer, "h3: request body read timed out");
                write_response(&mut stream, problem_response(504, GATEWAY_TIMEOUT_BODY)).await;
                return;
            }
            Err(BodyReadError::Stream(error)) => {
                tracing::debug!(%peer, error = %error, "h3: failed to read the request body");
                return;
            }
        };
        (body, None)
    };

    let mut transport_request = TransportRequest::new(parts, body, Some(peer));
    if let Some(fingerprint) = client_cert_fingerprint {
        transport_request = transport_request.with_client_cert_fingerprint(fingerprint);
    }
    let upgrade_tx = match upgrade {
        Some((upgrade, io_tx)) => {
            transport_request = transport_request.with_upgrade(upgrade);
            Some(io_tx)
        }
        None => None,
    };

    let response = match dispatch_request(server, transport_request, request_deadline).await {
        Ok(response) => response,
        Err(()) => {
            tracing::debug!(
                %peer,
                timeout = ?request_timeout,
                "h3: request dispatch timed out"
            );
            write_response(&mut stream, problem_response(504, GATEWAY_TIMEOUT_BODY)).await;
            return;
        }
    };
    match upgrade_tx {
        Some(io_tx) if response.status == 101 => {
            serve_websocket(stream, response, io_tx, websocket_bridges).await;
        }
        Some(_) if (200..300).contains(&response.status) => {
            // A 2xx response on an extended CONNECT means "tunnel established".
            // If the engine did not produce an upgrade, never forward a normal
            // body with tunnel semantics; answer as a missing WebSocket route.
            write_response(&mut stream, problem_response(404, NOT_FOUND_BODY)).await;
        }
        _ => write_response(&mut stream, response).await,
    }
}

/// Dispatch one request through the engine, enforcing the shared deadline.
///
/// `Err(())` means the deadline expired; the caller answers `504`.
async fn dispatch_request(
    server: Arc<webx_engine::Server>,
    request: TransportRequest,
    deadline: Option<tokio::time::Instant>,
) -> std::result::Result<TransportResponse, ()> {
    match deadline {
        Some(deadline) => {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            tokio::time::timeout(remaining, server.serve_transport_request(request))
                .await
                .map_err(|_| ())
        }
        None => Ok(server.serve_transport_request(request).await),
    }
}

/// Commit a successful extended CONNECT and bridge its HTTP/3 stream to a
/// duplex `UpgradedTransport` for the engine's `IUpgradeSession`.
async fn serve_websocket(
    mut stream: H3RequestStream,
    response: TransportResponse,
    io_tx: oneshot::Sender<UpgradedTransport>,
    websocket_bridges: Arc<AtomicUsize>,
) {
    let mut head = http::Response::new(());
    // RFC 9220: a successful extended CONNECT uses `200`, not the HTTP/1.1
    // `101 Switching Protocols`.
    *head.status_mut() = http::StatusCode::OK;
    for (name, value) in &response.headers {
        if is_hop_by_hop_header(name) {
            continue;
        }
        head.headers_mut().append(name.clone(), value.clone());
    }
    if let Err(error) = stream.send_response(head).await {
        tracing::debug!(error = %error, "h3: failed to commit the WebSocket extended CONNECT");
        return;
    }

    let (send_half, recv_half) = stream.split();
    let (session_io, bridge_io) = tokio::io::duplex(64 * 1024);
    let (bridge_read, bridge_write) = tokio::io::split(bridge_io);

    // Hand the session its byte stream first: if the engine session task was
    // never spawned (or has already gone away), do not leak the bridge pumps.
    let transport: UpgradedTransport = Box::new(session_io);
    if io_tx.send(transport).is_err() {
        tracing::debug!("h3: WebSocket session dropped before the stream bridge started");
        return;
    }

    let (mut send_half, mut recv_half) = (send_half, recv_half);
    let (mut bridge_read, mut bridge_write) = (bridge_read, bridge_write);

    // Track the bridge for graceful drain: the send pump writes the WebSocket
    // close frame, so the connection must wait for it before dropping h3.
    websocket_bridges.fetch_add(1, Ordering::SeqCst);
    let bridge_guard = WebSocketBridgeGuard(Arc::clone(&websocket_bridges));

    // h3 → engine session.
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

    // engine session → h3.
    tokio::spawn(async move {
        let _bridge_guard = bridge_guard;
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
}

/// Answer a request rejected before the engine sees it.
fn problem_response(status: u16, body: &'static [u8]) -> TransportResponse {
    let mut response = TransportResponse::new(status);
    response.headers.insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/problem+json"),
    );
    response.body = TransportResponseBody::Bytes(Bytes::from_static(body));
    response
}

/// Why reading a request body stopped.
enum BodyReadError {
    /// The HTTP/3 stream failed.
    Stream(h3::error::StreamError),
    /// The engine's request deadline expired while the client was still sending.
    Timeout,
}

/// Read the full request body, or `Ok(None)` when it exceeds `max_bytes`.
///
/// The optional `deadline` is the engine's request deadline; the client must
/// deliver the complete body before it, or the transport answers `504` instead
/// of buffering forever.
async fn read_request_body(
    stream: &mut H3RequestStream,
    max_bytes: usize,
    deadline: Option<tokio::time::Instant>,
) -> std::result::Result<Option<Bytes>, BodyReadError> {
    let mut body = Vec::new();
    loop {
        let next = match deadline {
            Some(deadline) => match tokio::time::timeout_at(deadline, stream.recv_data()).await {
                Ok(result) => result.map_err(BodyReadError::Stream)?,
                Err(_) => return Err(BodyReadError::Timeout),
            },
            None => stream.recv_data().await.map_err(BodyReadError::Stream)?,
        };
        let Some(mut chunk) = next else {
            break;
        };
        let length = chunk.remaining();
        if body.len().saturating_add(length) > max_bytes {
            return Ok(None);
        }
        body.extend_from_slice(&chunk.copy_to_bytes(length));
    }
    Ok(Some(Bytes::from(body)))
}

/// Answer a request rejected for exceeding the body cap.
async fn respond_payload_too_large(
    stream: &mut H3RequestStream,
) -> std::result::Result<(), h3::error::StreamError> {
    // The body will not be consumed: tell the peer to stop sending.
    stream.stop_sending(h3::error::Code::H3_REQUEST_CANCELLED);

    let mut head = http::Response::new(());
    *head.status_mut() = http::StatusCode::PAYLOAD_TOO_LARGE;
    head.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/problem+json"),
    );
    stream.send_response(head).await?;
    stream
        .send_data(Bytes::from_static(PAYLOAD_TOO_LARGE_BODY))
        .await?;
    stream.finish().await
}

/// Write an engine response as HTTP/3 response headers plus body frames.
async fn write_response(stream: &mut H3RequestStream, response: TransportResponse) {
    if let Err(error) = write_response_inner(stream, response).await {
        tracing::debug!(error = %error, "h3: failed to write the response");
    }
}

async fn write_response_inner(
    stream: &mut H3RequestStream,
    response: TransportResponse,
) -> std::result::Result<(), h3::error::StreamError> {
    let TransportResponse {
        status,
        headers,
        body,
    } = response;

    let mut head = http::Response::new(());
    *head.status_mut() =
        http::StatusCode::from_u16(status).unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR);
    for (name, value) in &headers {
        // HTTP/3 owns framing; connection-specific fields from the HTTP/1.1
        // world must never reach the wire (RFC 9114 §4.2).
        if is_hop_by_hop_header(name) {
            continue;
        }
        head.headers_mut().append(name.clone(), value.clone());
    }
    stream.send_response(head).await?;

    match body {
        TransportResponseBody::None => {}
        TransportResponseBody::Bytes(bytes) => {
            if !bytes.is_empty() {
                stream.send_data(bytes).await?;
            }
        }
        TransportResponseBody::Stream(mut body) => {
            while let Some(chunk) = body.next().await {
                match chunk {
                    Ok(bytes) => {
                        if !bytes.is_empty() {
                            stream.send_data(bytes).await?;
                        }
                    }
                    Err(error) => {
                        // The engine's stream failed mid-response: reset the
                        // stream so the peer sees an aborted response instead of
                        // a truncated one. Dropping the `RequestStream` is not
                        // enough — dropping a `quinn::SendStream` finishes it
                        // (clean FIN), which would make truncation look complete.
                        tracing::debug!(
                            error = %error,
                            "h3: response body stream failed; resetting the response stream"
                        );
                        stream.stop_stream(h3::error::Code::H3_INTERNAL_ERROR);
                        return Ok(());
                    }
                }
            }
        }
        _ => {}
    }

    stream.finish().await
}

/// Connection-specific header fields that HTTP/2 and HTTP/3 forbid.
fn is_hop_by_hop_header(name: &http::HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-connection"
            | "transfer-encoding"
            | "upgrade"
            | "te"
            | "trailer"
    )
}

/// SHA-256 of the verified client certificate (leaf DER), lowercase hex.
///
/// `quinn` surfaces the rustls peer chain as `Vec<CertificateDer<'static>>`;
/// the fingerprint format matches the engine's hyper/TLS path exactly, so the
/// `x-webx-client-cert-sha256` header contract is identical for both transports.
fn client_cert_fingerprint(connection: &quinn::Connection) -> Option<Arc<str>> {
    let identity = connection.peer_identity()?;
    let certificates = identity
        .downcast::<Vec<rustls::pki_types::CertificateDer<'static>>>()
        .ok()?;
    let leaf = certificates.first()?;

    let digest = Sha256::digest(leaf.as_ref());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Some(Arc::from(hex))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn self_signed_files() -> (tempfile::NamedTempFile, tempfile::NamedTempFile) {
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("self-signed certificate");
        let mut cert = tempfile::NamedTempFile::new().expect("cert file");
        cert.write_all(certified.cert.pem().as_bytes())
            .expect("write cert");
        let mut key = tempfile::NamedTempFile::new().expect("key file");
        key.write_all(certified.key_pair.serialize_pem().as_bytes())
            .expect("write key");
        (cert, key)
    }

    fn config(cert: &tempfile::NamedTempFile, key: &tempfile::NamedTempFile) -> H3Config {
        H3Config::new(
            "127.0.0.1:0",
            cert.path().to_path_buf(),
            key.path().to_path_buf(),
        )
    }

    #[test]
    fn a_non_socket_bind_address_is_a_readable_error() {
        let (cert, key) = self_signed_files();
        let error =
            H3Transport::new(H3Config::new("localhost:4433", cert.path(), key.path())).unwrap_err();
        assert!(
            error.to_string().contains("localhost:4433")
                && error.to_string().contains("not an IP:port socket address"),
            "{error}"
        );
    }

    #[test]
    fn a_missing_certificate_is_a_readable_error() {
        let (_, key) = self_signed_files();
        let error = H3Transport::new(H3Config::new(
            "127.0.0.1:0",
            "does-not-exist/cert.pem",
            key.path(),
        ))
        .unwrap_err();
        assert!(
            error.to_string().contains("does-not-exist/cert.pem"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn construction_binds_port_zero_and_shutdown_closes_cleanly() {
        let (cert, key) = self_signed_files();
        let transport = H3Transport::new(config(&cert, &key)).expect("transport builds");
        assert_eq!(transport.kind(), "h3");
        assert!(transport.local_addr().ip().is_loopback());
        assert_ne!(
            transport.local_addr().port(),
            0,
            "the OS-assigned port is reported"
        );
        // Closing an endpoint with no accepted connections must terminate
        // promptly and keep returning the same bound address.
        let address = transport.local_addr();
        transport.shutdown_gracefully().await;
        assert_eq!(address, transport.local_addr());
    }

    #[test]
    fn hop_by_hop_headers_are_identified() {
        for name in [
            "connection",
            "keep-alive",
            "proxy-connection",
            "transfer-encoding",
            "upgrade",
            "te",
            "trailer",
        ] {
            assert!(is_hop_by_hop_header(
                &http::HeaderName::from_bytes(name.as_bytes()).expect("header name")
            ));
        }
        for name in ["content-type", "content-length", "set-cookie", "date"] {
            assert!(!is_hop_by_hop_header(
                &http::HeaderName::from_bytes(name.as_bytes()).expect("header name")
            ));
        }
    }
}
