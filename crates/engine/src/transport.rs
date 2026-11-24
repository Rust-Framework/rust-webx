//! Request-level transport port.
//!
//! The engine historically spoke hyper directly: one `hyper::Request<Incoming>`
//! in, one `hyper::Response<RespBody>` out. This module defines the neutral
//! seam between **the engine** (router, middleware, handlers, error writers,
//! metrics, panic boundary) and **a protocol stack** (hyper HTTP/1.1+2 today,
//! HTTP/3/WebTransport through an optional adapter), so a transport can reuse
//! the complete pipeline without going through hyper or TCP.
//!
//! # Scope
//!
//! * Request bodies are neutral frame streams (`TransportRequestBody`) with an
//!   empty/buffered/streaming distinction; the hyper adapter feeds
//!   `hyper::body::Incoming` through the same representation.
//! * Response bodies are neutral too (`TransportResponseBody::{None,Bytes,Stream}`);
//!   the hyper path still produces its byte-identical response directly, while
//!   non-hyper transports receive the existing finalize/SSE lifecycle wrapped
//!   as a neutral byte stream.
//! * Protocol upgrades are neutral too: a request may carry a
//!   [`TransportUpgrade`] future whose `io` resolves after the transport commits
//!   the handshake response; the existing `IUpgradeSession` runs on the resolved
//!   byte stream (hyper/WebSocket today, HTTP/3 extended CONNECT later).
//!
//! # Application surface
//!
//! Nothing here changes the application-facing `webx` API: `IHttpContext`,
//! `IRequest`/`IRequestHandler`, middleware, DI, configuration and error types
//! are exactly the same. A transport implementation only moves `http` parts,
//! body frames, a peer address and headers.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use futures_util::Stream;
use http::request::Parts;
use http::HeaderMap;
use tokio::sync::watch;
use webx_core::error::{Error, Result};

use crate::shared::Shared;

/// Error carried by one request-body frame.
///
/// The engine maps it to its own error categories at the body-reading sites, so
/// the transport's error text (for example hyper's connection error) is
/// preserved in logs/`problem+json` details exactly like before.
pub type TransportBodyError = Box<dyn std::error::Error + Send + Sync>;

/// One request-body frame.
///
/// Trailers are represented explicitly so buffered readers can ignore them
/// (as before), while proxying readers can reject them (as before); collapsing
/// trailers into an error would change one of those two behaviors.
#[non_exhaustive]
pub enum TransportBodyFrame {
    /// Payload bytes.
    Data(Bytes),
    /// Trailing headers (not payload).
    Trailers(HeaderMap),
}

/// A request body as a frame stream.
///
/// The stream is `Send + Sync`: `EngineRequest` preserved that auto trait before
/// the transport port existed, and removing it in a minor release would be a
/// breaking change. A transport-native body that is only `Send` (hyper's
/// `Incoming`, h3 request streams) is wrapped once by
/// `sync_body_stream`, so the hot path pays one box and a per-frame lock —
/// never a second box or an extra dynamic dispatch.
pub type TransportRequestBodyStream = Pin<Box<dyn Stream<Item = RawBodyFrame> + Send + Sync>>;

/// One frame of a request body stream.
type RawBodyFrame = std::result::Result<TransportBodyFrame, TransportBodyError>;

/// Adapt a `Send`-only frame stream into the public `Send + Sync` stream.
///
/// Only the engine's single consumer polls this stream; the mutex exists to
/// satisfy the public `Sync` auto trait (and therefore `EngineRequest: Sync`),
/// not to share the stream between consumers. The adapter box is the only
/// allocation: the transport-native stream is stored inline.
pub(crate) struct SyncBodyStream<F> {
    inner: std::sync::Mutex<F>,
}

impl<F> SyncBodyStream<F> {
    pub(crate) fn new(frames: F) -> Self {
        Self {
            inner: std::sync::Mutex::new(frames),
        }
    }
}

impl<F> Stream for SyncBodyStream<F>
where
    F: Stream<Item = RawBodyFrame> + Unpin,
{
    type Item = RawBodyFrame;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        // `F: Unpin`, so the inner stream can be polled through a plain `&mut`.
        let this = self.get_mut();
        let mut frames = this
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Pin::new(&mut *frames).poll_next(cx)
    }
}

/// Wrap a `Send`-only stream into the public `Send + Sync` request stream.
pub(crate) fn sync_body_stream<F>(frames: F) -> TransportRequestBodyStream
where
    F: Stream<Item = RawBodyFrame> + Send + Unpin + 'static,
{
    Box::pin(SyncBodyStream::new(frames))
}

/// The request body as seen by the engine.
#[non_exhaustive]
pub enum TransportRequestBody {
    /// Known to carry no body (for example a GET with no payload).
    Empty,
    /// Fully buffered, replayable body (in-process fake transports and `from_parts`).
    Buffered(Bytes),
    /// Live frame stream with an optional exact length (`Content-Length`).
    Streaming {
        /// Payload/trailer frames.
        frames: TransportRequestBodyStream,
        /// Declared length when the protocol can provide one.
        exact_len: Option<u64>,
    },
}

impl TransportRequestBody {
    /// A buffered body.
    pub fn buffered(body: impl Into<Bytes>) -> Self {
        Self::Buffered(body.into())
    }

    /// A live frame stream.
    pub fn streaming(frames: TransportRequestBodyStream, exact_len: Option<u64>) -> Self {
        Self::Streaming { frames, exact_len }
    }
}

impl std::fmt::Debug for TransportRequestBody {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("Empty"),
            Self::Buffered(bytes) => formatter
                .debug_tuple("Buffered")
                .field(&format_args!("{} bytes", bytes.len()))
                .finish(),
            Self::Streaming { exact_len, .. } => formatter
                .debug_struct("Streaming")
                .field("exact_len", exact_len)
                .finish_non_exhaustive(),
        }
    }
}

/// Error returned while resolving an upgraded transport.
pub type TransportUpgradeError = Box<dyn std::error::Error + Send + Sync>;

/// A committed protocol upgrade (WebSocket today, WebTransport later).
///
/// `io` must not resolve before the transport has committed the handshake
/// response (HTTP/1.1 `101` / HTTP/3 extended CONNECT `200`); the engine waits
/// for it only after the response is finalized, and runs the existing
/// [`webx_core::realtime::IUpgradeSession`] on the byte stream.
pub struct TransportUpgrade {
    /// Resolves to the upgraded byte stream once the handshake is committed.
    pub io: Pin<
        Box<
            dyn Future<Output = std::result::Result<UpgradedTransport, TransportUpgradeError>>
                + Send,
        >,
    >,
}

impl std::fmt::Debug for TransportUpgrade {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TransportUpgrade")
            .finish_non_exhaustive()
    }
}

/// The upgraded byte stream handed to an `IUpgradeSession`.
pub type UpgradedTransport = webx_core::realtime::UpgradedTransport;

/// Engine state handed to an [`IHttpTransport`] listener backend.
///
/// The backend owns accept and transport-level handshakes; the engine owns
/// request dispatch and the process-wide connection governor. Use
/// [`Self::admit_connection`] for every accepted connection and
/// [`Self::server`] (or [`Self::serve_request`](crate::Server::serve_transport_request))
/// to serve neutral requests.
#[derive(Clone)]
pub struct TransportRuntime {
    shared: Arc<Shared>,
    shutdown: watch::Receiver<bool>,
    transport_metrics: Option<Arc<crate::shared::TransportMetrics>>,
}

impl TransportRuntime {
    /// Build a runtime from the assembled engine state and shutdown signal.
    pub(crate) fn new(shared: Arc<Shared>, shutdown: watch::Receiver<bool>) -> Self {
        Self {
            shared,
            shutdown,
            transport_metrics: None,
        }
    }

    /// Attach the per-transport counters registered for this backend.
    pub(crate) fn with_transport_metrics(
        mut self,
        metrics: Arc<crate::shared::TransportMetrics>,
    ) -> Self {
        self.transport_metrics = Some(metrics);
        self
    }

    /// Per-transport listener counters (registered by `run_transport`).
    pub fn transport_metrics(&self) -> Option<&Arc<crate::shared::TransportMetrics>> {
        self.transport_metrics.as_ref()
    }

    /// The shared engine state (router, pipeline, limits, DI handles, ...).
    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// Engine server bound to this shared state.
    ///
    /// The backend uses it to submit each decoded request through
    /// [`crate::Server::serve_transport_request`].
    pub fn server(&self) -> crate::Server {
        crate::Server::new(Arc::clone(&self.shared))
    }

    /// A fresh receiver for the engine's graceful-shutdown signal.
    pub fn subscribe_shutdown(&self) -> watch::Receiver<bool> {
        self.shared.subscribe_shutdown()
    }

    /// Whether graceful shutdown has started.
    pub fn is_shutting_down(&self) -> bool {
        *self.shutdown.borrow()
    }

    /// Admit one transport connection against the engine's global governor.
    ///
    /// QUIC/UDP backends cannot write an HTTP `503` before the handshake, so
    /// they call this after the transport handshake: the call waits up to
    /// `Server.AdmissionTimeoutMs` for an `App.MaxConnections` slot, records the
    /// same admitted/rejected/active counters as the TCP acceptors, and returns
    /// a guard the backend holds for the connection's lifetime. On timeout or
    /// engine shutdown the backend must close the connection without serving
    /// requests.
    ///
    /// The wait is cancellation-safe: the queue-depth gauge is released on every
    /// exit (permit granted, timeout, shutdown, or the backend aborting the
    /// future), and engine shutdown ends the wait even when
    /// `Server.AdmissionTimeoutMs` is `0` (wait indefinitely).
    pub async fn admit_connection(&self) -> Result<TransportConnectionPermit> {
        // Subscribe before reading the current value: a shutdown that lands
        // between the two steps would otherwise be missed (and `changed()` would
        // never fire again), leaving an unbounded wait with
        // `Server.AdmissionTimeoutMs = 0`.
        let mut shutdown = self.shared.subscribe_shutdown();
        if *shutdown.borrow() {
            return Err(Error::Http("server is shutting down".to_string()));
        }
        let stats = Arc::clone(&self.shared.admission);
        let governor = stats.governor(self.shared.config.max_connections);
        // RAII: an aborted wait (connection task cancelled/aborted) must still
        // pair the queue-depth counter, or the invariant gauge stays non-zero.
        let waiting = AdmissionWait::new(Arc::clone(&stats));
        let admission = match self.shared.config.admission_timeout {
            Some(timeout) => {
                tokio::select! {
                    biased;
                    _ = shutdown.changed() => TransportAdmission::ShuttingDown,
                    outcome = tokio::time::timeout(timeout, Arc::clone(&governor).acquire_owned()) => {
                        match outcome {
                            Ok(Ok(permit)) => TransportAdmission::Permit(permit),
                            Ok(Err(_closed)) => TransportAdmission::Closed,
                            Err(_) => TransportAdmission::TimedOut,
                        }
                    }
                }
            }
            None => {
                tokio::select! {
                    biased;
                    _ = shutdown.changed() => TransportAdmission::ShuttingDown,
                    outcome = Arc::clone(&governor).acquire_owned() => match outcome {
                        Ok(permit) => TransportAdmission::Permit(permit),
                        Err(_closed) => TransportAdmission::Closed,
                    },
                }
            }
        };
        drop(waiting);
        match admission {
            TransportAdmission::Permit(permit) => {
                stats.record_admitted();
                Ok(TransportConnectionPermit::new(permit, stats))
            }
            TransportAdmission::TimedOut => {
                stats.record_rejected();
                Err(Error::Http("connection admission timed out".to_string()))
            }
            TransportAdmission::ShuttingDown => {
                Err(Error::Http("server is shutting down".to_string()))
            }
            TransportAdmission::Closed => {
                Err(Error::Internal("connection governor closed".to_string()))
            }
        }
    }
}

/// Keeps the admission queue-depth gauge paired with [`AdmissionWait::new`].
struct AdmissionWait(Arc<crate::shared::AdmissionStats>);

impl AdmissionWait {
    fn new(stats: Arc<crate::shared::AdmissionStats>) -> Self {
        stats.begin_wait();
        Self(stats)
    }
}

impl Drop for AdmissionWait {
    fn drop(&mut self) {
        self.0.end_wait();
    }
}

/// One transport connection's admission slot.
///
/// Holds the shared `App.MaxConnections` permit and keeps the process-wide
/// active-connection gauge accurate; dropping it releases the slot.
pub struct TransportConnectionPermit {
    _permit: tokio::sync::OwnedSemaphorePermit,
    stats: Arc<crate::shared::AdmissionStats>,
}

impl std::fmt::Debug for TransportConnectionPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TransportConnectionPermit")
    }
}

impl TransportConnectionPermit {
    fn new(
        permit: tokio::sync::OwnedSemaphorePermit,
        stats: Arc<crate::shared::AdmissionStats>,
    ) -> Self {
        stats.record_connection_open();
        Self {
            _permit: permit,
            stats,
        }
    }
}

impl Drop for TransportConnectionPermit {
    fn drop(&mut self) {
        self.stats.record_connection_closed();
    }
}

enum TransportAdmission {
    Permit(tokio::sync::OwnedSemaphorePermit),
    TimedOut,
    ShuttingDown,
    Closed,
}

/// Listener-level transport backend (UDP/QUIC endpoint, custom accept loop, ...).
///
/// The backend owns the listener, protocol handshakes and connection lifecycle,
/// and submits each decoded request to the engine through
/// [`crate::Server::serve_transport_request`] (multiplexed backends spawn one
/// task per request stream). The hyper HTTP/1.1+2 listener keeps its existing
/// worker model internally; this trait is the additive seam for new transport
/// models (HTTP/3, in-process test listeners).
#[async_trait::async_trait]
pub trait IHttpTransport: Send + Sync {
    /// Short transport label for logs/metrics (for example `"h3"`, `"in-process"`).
    fn kind(&self) -> &'static str;

    /// How long the host waits for this backend to stop after the shutdown
    /// signal fired.
    ///
    /// The backend owns its protocol-level drain (in-flight requests, active
    /// sessions, endpoint idle); the host aborts it after this budget. The
    /// default covers backends with a bounded drain of a few tens of seconds;
    /// a backend that needs longer overrides this.
    fn stop_budget(&self) -> std::time::Duration {
        std::time::Duration::from_secs(30)
    }

    /// Run the listener until shutdown, serving connections through
    /// [`TransportRuntime::server`].
    async fn run(self: Arc<Self>, runtime: TransportRuntime) -> Result<()>;
}

/// One HTTP request handed to the engine, independent of the wire protocol.
///
/// `parts` is the standard `http` crate representation (the same type hyper
/// uses), so the hyper adapter is a move rather than a copy.
pub struct TransportRequest {
    /// Method, URI, version and headers.
    pub parts: Parts,
    /// Request body (empty, buffered or a live frame stream).
    pub body: TransportRequestBody,
    /// Peer address as observed by the transport (QUIC remote address, TCP
    /// peer, in-process loopback in tests).
    pub peer: Option<SocketAddr>,
    /// Verified TLS client-certificate fingerprint supplied by the transport's
    /// TLS terminator.
    ///
    /// The engine removes any client-supplied
    /// `x-webx-client-cert-sha256` header first and inserts this verified value
    /// only when present, so a transport adapter (h3/QUIC included) follows the
    /// same "TLS terminator is the only writer" contract as the hyper path.
    pub client_cert_fingerprint: Option<Arc<str>>,
    /// Protocol upgrade carried by this request, if any.
    pub upgrade: Option<TransportUpgrade>,
}

impl std::fmt::Debug for TransportRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TransportRequest")
            .field("method", &self.parts.method)
            .field("uri", &self.parts.uri)
            .field("body", &self.body)
            .field("peer", &self.peer)
            .field("client_cert_fingerprint", &self.client_cert_fingerprint)
            .field("upgrade", &self.upgrade)
            .finish()
    }
}

impl TransportRequest {
    /// Build a request with an in-memory body.
    pub fn new(parts: Parts, body: impl Into<Bytes>, peer: Option<SocketAddr>) -> Self {
        Self {
            parts,
            body: TransportRequestBody::Buffered(body.into()),
            peer,
            client_cert_fingerprint: None,
            upgrade: None,
        }
    }

    /// Build a request from an explicit neutral body.
    pub fn with_body(parts: Parts, body: TransportRequestBody, peer: Option<SocketAddr>) -> Self {
        Self {
            parts,
            body,
            peer,
            client_cert_fingerprint: None,
            upgrade: None,
        }
    }

    /// Attach the verified client-certificate fingerprint for this request.
    pub fn with_client_cert_fingerprint(mut self, fingerprint: impl Into<Arc<str>>) -> Self {
        self.client_cert_fingerprint = Some(fingerprint.into());
        self
    }

    /// Attach the protocol upgrade carried by this request.
    pub fn with_upgrade(mut self, upgrade: TransportUpgrade) -> Self {
        self.upgrade = Some(upgrade);
        self
    }
}

/// A response body as seen by the engine.
#[non_exhaustive]
pub enum TransportResponseBody {
    /// No body (HEAD / 204 / 304 / 1xx).
    None,
    /// Fully buffered body (JSON, small problem responses).
    Bytes(Bytes),
    /// Streaming body (files, SSE, unknown-length streams).
    Stream(TransportResponseBodyStream),
}

/// A response body as a byte stream.
pub type TransportResponseBodyStream = Pin<Box<dyn Stream<Item = std::io::Result<Bytes>> + Send>>;

impl std::fmt::Debug for TransportResponseBody {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => formatter.write_str("None"),
            Self::Bytes(bytes) => formatter
                .debug_tuple("Bytes")
                .field(&format_args!("{} bytes", bytes.len()))
                .finish(),
            Self::Stream(_) => formatter.write_str("Stream"),
        }
    }
}

/// One HTTP response produced by the engine, independent of the wire protocol.
///
/// Status and headers are the exact values the hyper path would have written
/// (including content-type, security headers set by middleware, and the
/// `problem+json` error representation). `body` is buffered or streamed; the
/// transport owns framing (HTTP/1.1 chunked, HTTP/2/3 DATA frames, ...).
pub struct TransportResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers; duplicate values (`Set-Cookie`) are preserved.
    pub headers: HeaderMap,
    /// Response body.
    pub body: TransportResponseBody,
}

impl std::fmt::Debug for TransportResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TransportResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("body", &self.body)
            .finish()
    }
}

impl TransportResponse {
    /// An empty response with `status`.
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: TransportResponseBody::None,
        }
    }

    /// A response with an in-memory body.
    pub fn bytes(status: u16, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: TransportResponseBody::Bytes(body.into()),
        }
    }

    /// Read one response header (case-insensitive, first value).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    /// Whether this response carries a streaming body.
    pub fn is_streaming(&self) -> bool {
        matches!(self.body, TransportResponseBody::Stream(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send<T: Send>() {}

    #[test]
    fn neutral_types_are_send() {
        assert_send::<TransportRequest>();
        assert_send::<TransportResponse>();
    }

    #[test]
    fn response_header_lookup_is_case_insensitive() {
        let mut response = TransportResponse::new(200);
        response.headers.insert(
            http::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        assert_eq!(response.header("Content-Type"), Some("application/json"));
        assert_eq!(response.header("missing"), None);
    }
}
