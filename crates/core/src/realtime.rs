//! Real-time connection abstractions (WebSocket first; SSE shares the metadata).
//!
//! The engine (HTTP wire) owns the protocol switch; the framework owns the
//! session contract. An endpoint or middleware that decides to accept an
//! upgrade calls [`IHttpContext::set_upgrade_session`], writes the `101`
//! response, and the transport backend then invokes
//! [`IUpgradeSession::run`] with the raw upgraded byte stream.
//!
//! [`IHttpContext::set_upgrade_session`]: crate::http::IHttpContext::set_upgrade_session

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::Result;

pub mod event_store;
pub mod fanout;
pub mod metrics;
pub mod registry;
pub mod signaling;

pub use event_store::{
    IRealtimeEventStore, InMemoryRealtimeEventStore, RealtimeEvent, RealtimeEventStoreLimits,
    ResumeFrame, ResumeRequest,
};
pub use fanout::RealtimeFanoutEvent;
pub use metrics::{RealtimeMetrics, RealtimeMetricsSnapshot};
pub use registry::{
    IConnectionRegistry, IPresenceObserver, InMemoryConnectionRegistry, Presence,
    PresenceDirection, PresenceStatus, PresenceTransition,
};
pub use signaling::{
    hmac_sha1_base64, SignalingKind, SignalingMessage, TurnCredentials, TurnRestConfig,
    DEFAULT_TURN_TTL_SECONDS, MAX_TURN_TTL_SECONDS,
};

/// An upgraded, protocol-agnostic byte stream (WebSocket frames are written by
/// the session, not by the engine).
///
/// The supertrait bundle exists because Rust trait objects cannot combine two
/// non-auto traits (`AsyncRead + AsyncWrite`); the blanket impl makes every
/// compatible stream a [`Transport`].
pub trait Transport: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static {}

impl<T> Transport for T where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static
{
}

/// Boxed upgraded transport handed to [`IUpgradeSession::run`].
pub type UpgradedTransport = Box<dyn Transport>;

/// Process-unique connection identity.
///
/// Monotonic and cheap: generated once per connection, used for logs, metrics
/// and (later) the `IConnectionRegistry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionId(u64);

impl ConnectionId {
    /// Allocate the next id.
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// Raw value (logs / tests).
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "c{}", self.0)
    }
}

/// Which wire transport carries the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    /// Plain HTTP request/response (short-lived `IConnection` view if used).
    Http,
    /// A WebSocket upgrade.
    WebSocket,
    /// An SSE stream.
    Sse,
    /// One HTTP/2 stream (multiplexed).
    Http2Stream,
}

impl TransportKind {
    /// Stable label for metrics/logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::WebSocket => "websocket",
            Self::Sse => "sse",
            Self::Http2Stream => "h2-stream",
        }
    }
}

/// Immutable metadata for one connection.
#[derive(Debug, Clone)]
pub struct ConnectionMeta {
    /// Connection identity.
    pub id: ConnectionId,
    /// Transport kind.
    pub transport: TransportKind,
    /// Client address as reported by the engine (`None` when unavailable).
    pub remote_addr: Option<String>,
    /// Trace id of the upgrade request, propagated to the session (logs and
    /// `RequestContext::trace_id()` inside message handlers).
    pub trace_id: Option<String>,
}

impl ConnectionMeta {
    /// New metadata with a fresh id.
    pub fn new(transport: TransportKind) -> Self {
        Self {
            id: ConnectionId::next(),
            transport,
            remote_addr: None,
            trace_id: None,
        }
    }

    /// Attach the client address.
    pub fn with_remote_addr(mut self, remote_addr: impl Into<String>) -> Self {
        self.remote_addr = Some(remote_addr.into());
        self
    }

    /// Attach the upgrade request's trace id.
    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }
}

/// One application-level real-time message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeMessage {
    /// UTF-8 text frame.
    Text(String),
    /// Binary frame.
    Binary(Vec<u8>),
    /// Close frame from the peer.
    Close {
        /// WebSocket close code.
        code: u16,
        /// Optional reason.
        reason: String,
    },
}

/// Why the framework ended a real-time connection.
///
/// The framework records the reason on the connection before writing the
/// closing frame, so application code, presence observers and tests can tell an
/// idle timeout from a deploy without parsing the wire code:
/// [`IConnection::close_reason`] returns it, and each variant maps to the RFC
/// 6455 close code the server sends ([`CloseReason::code`]). The table of
/// reasons, wire codes and SSE counterparts lives in `docs/design/close-codes.md`.
///
/// `#[non_exhaustive]`: new reasons are additive in 2.x.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CloseReason {
    /// The peer closed the connection: it sent a close frame, ended the stream
    /// or the transport failed. The peer's own close code is delivered to the
    /// application as [`RealtimeMessage::Close`]; this reason only says who
    /// initiated.
    ClientClosed,
    /// The host started graceful shutdown and the session ended proactively
    /// with code `1001` ("going away") instead of being cut at the drain
    /// deadline.
    ServerShutdown,
    /// The application handler returned; the framework closed normally (`1000`).
    SessionCompleted,
    /// No inbound frame arrived within [`WebSocketLimits::idle_timeout`]
    /// (code `1001`, "going away").
    IdleTimeout,
    /// [`WebSocketLimits::max_connection_lifetime`] elapsed (code `1001`).
    LifetimeExceeded,
    /// The session outlived the `exp` of the token that authorized the upgrade
    /// (code `1008`, policy violation). See
    /// [`WebSocketLimits::close_at_token_expiry`].
    TokenExpired,
    /// A protocol or policy violation closed the connection with `code` (for
    /// example the inbound rate limit, `1008`).
    ProtocolError {
        /// WebSocket close code the server sent.
        code: u16,
    },
    /// The session ended on an internal framework or handler error.
    Internal,
}

impl CloseReason {
    /// The WebSocket close code the server sends for this reason.
    ///
    /// `Option` keeps the contract additive: a future reason that is only
    /// observable in-process (no server-issued frame) can return `None` without
    /// changing the signature. Every reason defined today has a wire code.
    pub fn code(&self) -> Option<u16> {
        match self {
            Self::ClientClosed => Some(1000),
            Self::ServerShutdown => Some(1001),
            Self::SessionCompleted => Some(1000),
            Self::IdleTimeout => Some(1001),
            Self::LifetimeExceeded => Some(1001),
            Self::TokenExpired => Some(1008),
            Self::ProtocolError { code } => Some(*code),
            Self::Internal => Some(1011),
        }
    }
}

/// Per-connection limits for a real-time session (2.3-5).
///
/// Defaults are permissive (no rate limit, no idle timeout) so existing
/// sessions keep their behavior; deployments opt in through
/// `HostBuilder::use_websocket_limits`.
#[derive(Debug, Clone)]
pub struct WebSocketLimits {
    /// Maximum size of one inbound text/binary frame, in bytes.
    pub max_frame_bytes: usize,
    /// Inbound message rate limit per second (`0` = unlimited).
    pub max_messages_per_second: u32,
    /// Inbound byte rate limit per second (`0` = unlimited).
    pub max_bytes_per_second: u64,
    /// Outbound queue depth per connection (bounded backpressure).
    pub outbound_queue: usize,
    /// What to do when the outbound queue is full.
    pub on_queue_full: BackpressurePolicy,
    /// Close the connection when no inbound frame arrives within this duration.
    ///
    /// The session records [`CloseReason::IdleTimeout`] and closes with `1001`.
    pub idle_timeout: Option<std::time::Duration>,
    /// Absolute cap on how long one connection may live (`None` = unlimited).
    ///
    /// Unlike [`Self::idle_timeout`], traffic does not extend this deadline:
    /// the session is closed at `opened_at + lifetime` with close code `1001`
    /// and [`CloseReason::LifetimeExceeded`].
    pub max_connection_lifetime: Option<std::time::Duration>,
    /// Also close the session when the upgrade token's `exp` passes.
    ///
    /// Browser clients cannot refresh the credential used at upgrade time, so a
    /// session outliving its access token keeps an expired credential alive.
    /// When enabled (default `false`), the session closes with code `1008`,
    /// [`CloseReason::TokenExpired`], and the earlier of this and
    /// [`Self::max_connection_lifetime`] wins.
    pub close_at_token_expiry: bool,
}

impl Default for WebSocketLimits {
    fn default() -> Self {
        Self {
            max_frame_bytes: 1024 * 1024,
            max_messages_per_second: 0,
            max_bytes_per_second: 0,
            outbound_queue: 64,
            on_queue_full: BackpressurePolicy::Drop,
            idle_timeout: None,
            max_connection_lifetime: None,
            close_at_token_expiry: false,
        }
    }
}

impl WebSocketLimits {
    /// Default limits.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the inbound frame size cap.
    pub fn with_max_frame_bytes(mut self, bytes: usize) -> Self {
        self.max_frame_bytes = bytes;
        self
    }

    /// Allow at most `messages` inbound messages per second (`0` = unlimited).
    pub fn with_max_messages_per_second(mut self, messages: u32) -> Self {
        self.max_messages_per_second = messages;
        self
    }

    /// Allow at most `bytes` inbound bytes per second (`0` = unlimited).
    pub fn with_max_bytes_per_second(mut self, bytes: u64) -> Self {
        self.max_bytes_per_second = bytes;
        self
    }

    /// Bound the outbound queue.
    pub fn with_outbound_queue(mut self, queue: usize, policy: BackpressurePolicy) -> Self {
        self.outbound_queue = queue.max(1);
        self.on_queue_full = policy;
        self
    }

    /// Close connections that stay idle for `timeout`.
    pub fn with_idle_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.idle_timeout = Some(timeout);
        self
    }

    /// Cap how long one connection may live, regardless of traffic.
    pub fn with_max_connection_lifetime(mut self, lifetime: std::time::Duration) -> Self {
        self.max_connection_lifetime = Some(lifetime);
        self
    }

    /// Close the session when the upgrade token's `exp` passes.
    pub fn close_at_token_expiry(mut self) -> Self {
        self.close_at_token_expiry = true;
        self
    }
}

/// What to do when a connection's outbound queue is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackpressurePolicy {
    /// Drop the frame (best effort) and count it as a backpressure drop.
    Drop,
    /// Fail the send and close the connection.
    Disconnect,
}

/// Per-connection counters exposed to handlers and metrics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConnectionStats {
    /// Inbound messages delivered to the handler.
    pub messages_in: u64,
    /// Outbound messages queued successfully.
    pub messages_out: u64,
    /// Inbound payload bytes.
    pub bytes_in: u64,
    /// Outbound payload bytes.
    pub bytes_out: u64,
    /// Frames dropped because the outbound queue was full.
    pub dropped_outbound: u64,
    /// Times the inbound rate limit tripped.
    pub rate_limited: u64,
}

/// A live connection as seen by application code.
///
/// Implementations live in the framework (WebSocket session over the upgraded
/// transport). The trait is deliberately transport-neutral so handlers written
/// today work for WebSocket, SSE and (later) HTTP/2 streams.
#[async_trait]
pub trait IConnection: Send + Sync {
    /// Connection metadata (id, transport, peer, trace id).
    fn meta(&self) -> &ConnectionMeta;

    /// Connection-lifetime application context: identity (when authenticated),
    /// trace id and a connection-scoped DI resolver.
    fn context(&self) -> &ConnectionContext;

    /// Per-connection counters (in/out, drops, rate limits).
    fn stats(&self) -> ConnectionStats {
        ConnectionStats::default()
    }

    /// Why the framework ended this connection, when it recorded a reason.
    ///
    /// The framework's WebSocket session records the reason before it writes
    /// the close frame: lifetime cap, token expiry, idle timeout, server
    /// shutdown, a rate-limit close or the client going away. It stays `None`
    /// for an application-initiated [`Self::close`] (the wire code the
    /// application chose is authoritative there) and for connections that are
    /// still open.
    ///
    /// Default `None`: custom [`IConnection`] implementations do not need to
    /// track a reason (2.x additive).
    fn close_reason(&self) -> Option<CloseReason> {
        None
    }

    /// Shorthand for `self.meta().id`.
    fn id(&self) -> ConnectionId {
        self.meta().id
    }

    /// Receive the next message; `None` after the peer closes or the connection
    /// shuts down.
    async fn recv(&self) -> Option<Result<RealtimeMessage>>;

    /// Send a text message.
    async fn send_text(&self, text: String) -> Result<()>;

    /// Send a binary message.
    async fn send_binary(&self, bytes: Vec<u8>) -> Result<()>;

    /// Start a close handshake.
    async fn close(&self, code: u16, reason: &str) -> Result<()>;
}

/// A protocol session that runs on the upgraded transport.
///
/// The transport backend spawns this after the `101` response is committed;
/// the returned future completes when the connection ends.
#[async_trait]
pub trait IUpgradeSession: Send + Sync {
    /// Run the session until the peer or the server closes it.
    async fn run(&self, transport: UpgradedTransport, meta: ConnectionMeta) -> Result<()>;
}

/// Connection-lifetime DI scope.
///
/// Created once per accepted real-time connection and disposed when the session
/// ends: `Scoped` services resolved here live for the whole connection (one
/// cache per connection), and their disposables run on session teardown.
///
/// The underlying [`rust_dix::Scope`] is not `Clone` and must not be cached by
/// singletons; the framework owns it and only hands out `Arc`-resolved services.
pub struct ConnectionScope {
    scope: std::sync::Mutex<Option<rust_dix::Scope>>,
}

impl ConnectionScope {
    /// Wrap a freshly created DI scope.
    pub fn new(scope: rust_dix::Scope) -> Self {
        Self {
            scope: std::sync::Mutex::new(Some(scope)),
        }
    }

    /// Resolve a Scoped/Transient/Singleton service from the connection scope.
    ///
    /// Returns `None` when the service is not registered or the scope has
    /// already been disposed.
    pub fn resolve<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let guard = self.scope.lock().ok()?;
        guard.as_ref()?.get_optional::<T>()
    }

    /// Dispose the scope, running its registered cleanups in reverse order.
    pub fn dispose(&self) {
        let scope = self.scope.lock().ok().and_then(|mut guard| guard.take());
        if let Some(scope) = scope {
            scope.dispose();
        }
    }
}

impl Drop for ConnectionScope {
    fn drop(&mut self) {
        self.dispose();
    }
}

/// Per-connection application context.
///
/// Lives exactly as long as the connection. Message handlers resolve
/// connection-level state through [`Self::resolve`] (it comes from the
/// connection DI scope, so it is shared across messages of one connection and
/// isolated between connections).
#[derive(Clone)]
pub struct ConnectionContext {
    meta: ConnectionMeta,
    identity: Option<Arc<crate::auth::IdentityUser>>,
    scope: Arc<ConnectionScope>,
}

impl ConnectionContext {
    /// Build a context around an existing connection scope.
    pub fn new(
        meta: ConnectionMeta,
        identity: Option<Arc<crate::auth::IdentityUser>>,
        scope: Arc<ConnectionScope>,
    ) -> Self {
        Self {
            meta,
            identity,
            scope,
        }
    }

    /// Connection metadata.
    pub fn meta(&self) -> &ConnectionMeta {
        &self.meta
    }

    /// Authenticated identity captured at upgrade time (when available).
    pub fn identity(&self) -> Option<&Arc<crate::auth::IdentityUser>> {
        self.identity.as_ref()
    }

    /// Trace id of the upgrade request.
    pub fn trace_id(&self) -> Option<&str> {
        self.meta.trace_id.as_deref()
    }

    /// Resolve a service from the connection scope.
    pub fn resolve<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.scope.resolve::<T>()
    }
}

/// Application handler for one WebSocket connection.
///
/// Implementations own the connection's receive loop: call
/// [`IConnection::recv`] until it returns `None`, and send replies through the
/// same handle. Returning ends the session (the framework then closes the
/// socket).
#[async_trait]
pub trait IWebSocketHandler: Send + Sync {
    /// Run one accepted connection.
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()>;
}

/// Create a WebSocket `ConnectionMeta` without allocating an id twice.
pub fn websocket_meta(remote_addr: Option<String>) -> ConnectionMeta {
    let mut meta = ConnectionMeta::new(TransportKind::WebSocket);
    meta.remote_addr = remote_addr;
    meta
}

/// Share a session across the engine and the middleware that installs it.
pub type SessionRef = Arc<dyn IUpgradeSession>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_ids_are_unique_and_monotonic() {
        let first = ConnectionId::next();
        let second = ConnectionId::next();
        assert!(second.value() > first.value());
        assert_ne!(first, second);
    }

    #[test]
    fn meta_defaults_and_labels() {
        let meta = websocket_meta(Some("10.0.0.1:5555".to_string()));
        assert_eq!(meta.transport, TransportKind::WebSocket);
        assert_eq!(meta.remote_addr.as_deref(), Some("10.0.0.1:5555"));
        assert_eq!(TransportKind::WebSocket.as_str(), "websocket");
        assert!(matches!(
            ConnectionMeta::new(TransportKind::Sse).transport,
            TransportKind::Sse
        ));
    }

    #[test]
    fn close_reasons_expose_their_wire_codes() {
        assert_eq!(CloseReason::ClientClosed.code(), Some(1000));
        assert_eq!(CloseReason::ServerShutdown.code(), Some(1001));
        assert_eq!(CloseReason::SessionCompleted.code(), Some(1000));
        assert_eq!(CloseReason::IdleTimeout.code(), Some(1001));
        assert_eq!(CloseReason::LifetimeExceeded.code(), Some(1001));
        assert_eq!(CloseReason::TokenExpired.code(), Some(1008));
        assert_eq!(CloseReason::ProtocolError { code: 1008 }.code(), Some(1008));
        assert_eq!(CloseReason::Internal.code(), Some(1011));
    }
}
