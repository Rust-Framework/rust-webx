//! WebSocket sessions over the engine's upgraded transport (2.3-1b).
//!
//! [`WebSocketMiddleware`] matches upgrade requests against the routes
//! registered with `HostBuilder::use_websocket`, performs the RFC 6455
//! handshake (`101` + `Sec-WebSocket-Accept`), installs a session, and then
//! hands the raw upgraded stream to `tokio-tungstenite` for framing.
//! Application code only sees transport-neutral [`IConnection`] messages.

use std::marker::PhantomData;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use webx_core::error::{Error, Result};
use webx_core::http::IHttpContext;
use webx_core::mediator::{IMediator, IRequest, Mediator};
use webx_core::middleware::IMiddleware;
use webx_core::realtime::{
    websocket_meta, BackpressurePolicy, ConnectionContext, ConnectionMeta, ConnectionScope,
    ConnectionStats, IConnection, IUpgradeSession, IWebSocketHandler, RealtimeMessage,
    RealtimeMetrics, UpgradedTransport, WebSocketLimits,
};
use webx_core::request_context::RequestContext;

type SessionSocket = WebSocketStream<UpgradedTransport>;
type SessionSink = SplitSink<SessionSocket, Message>;
type SessionStream = SplitStream<SessionSocket>;

/// One WebSocket route: exact path plus the application handler.
#[derive(Clone)]
pub struct WebSocketRoute {
    /// Exact request path (query string ignored).
    pub path: &'static str,
    /// Application connection handler.
    pub handler: Arc<dyn IWebSocketHandler>,
}

/// Middleware that intercepts WebSocket upgrade requests.
///
/// Installed by the host when `use_websocket` registered at least one route;
/// requests that are not upgrades fall through to the normal HTTP pipeline.
pub struct WebSocketMiddleware {
    routes: Arc<[WebSocketRoute]>,
    /// Root provider; each accepted session gets its own connection scope.
    provider: Arc<rust_dix::ServiceProvider>,
    /// Per-connection limits copied into every accepted session.
    limits: WebSocketLimits,
}

impl WebSocketMiddleware {
    /// Build the middleware from the registered routes and the host provider.
    pub fn new(
        routes: Vec<WebSocketRoute>,
        provider: Arc<rust_dix::ServiceProvider>,
        limits: WebSocketLimits,
    ) -> Self {
        Self {
            routes: routes.into(),
            provider,
            limits,
        }
    }

    #[allow(dead_code)]
    fn matching_route(&self, path: &str) -> Option<&WebSocketRoute> {
        self.routes.iter().find(|route| route.path == path)
    }
}

fn is_upgrade_request(ctx: &dyn IHttpContext) -> bool {
    let upgrade = ctx
        .request()
        .header("upgrade")
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    if !upgrade {
        return false;
    }
    ctx.request().header("connection").is_some_and(|value| {
        value
            .split(',')
            .any(|part| part.trim().eq_ignore_ascii_case("upgrade"))
    })
}

#[async_trait]
impl IMiddleware for WebSocketMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        // Compare without allocating: the borrow ends before `ctx` is mutated.
        let Some(route) = self
            .routes
            .iter()
            .find(|route| route.path == ctx.request().path())
        else {
            return Ok(ControlFlow::Continue(()));
        };
        let handler = Arc::clone(&route.handler);

        if !is_upgrade_request(ctx) {
            // The path is a WebSocket endpoint, but the client sent a plain
            // request: answer 426 instead of routing it as a 404.
            ctx.response_mut().set_status(426);
            ctx.response_mut().set_header("upgrade", "websocket");
            ctx.response_mut().set_header("connection", "close");
            return Ok(ControlFlow::Break(()));
        }

        let Some(key) = ctx
            .request()
            .header("sec-websocket-key")
            .map(str::to_string)
        else {
            ctx.response_mut().set_status(400);
            ctx.response_mut()
                .set_header("content-type", "text/plain; charset=utf-8");
            return Ok(ControlFlow::Break(()));
        };
        let remote_addr = ctx.request().remote_addr().map(|addr| addr.to_string());
        let trace_id = ctx.trace_id().map(str::to_string);
        // The authentication middleware runs before this stage, so claims are
        // already available; capture the identity once for the whole session.
        let identity = ctx
            .claims()
            .map(|claims| Arc::new(webx_core::auth::IdentityUser::from_claims(claims)));
        let accept = derive_accept_key(key.as_bytes());

        ctx.response_mut().set_status(101);
        ctx.response_mut().set_header("connection", "Upgrade");
        ctx.response_mut().set_header("upgrade", "websocket");
        ctx.response_mut()
            .set_header("sec-websocket-accept", &accept);
        let mut meta = websocket_meta(remote_addr);
        if let Some(trace_id) = trace_id {
            meta = meta.with_trace_id(trace_id);
        }
        ctx.set_upgrade_session(
            Arc::new(WebSocketSession {
                handler,
                provider: Arc::clone(&self.provider),
                identity,
                limits: self.limits.clone(),
            }),
            meta,
        );
        Ok(ControlFlow::Break(()))
    }
}

/// Runs one WebSocket session on the upgraded transport.
struct WebSocketSession {
    handler: Arc<dyn IWebSocketHandler>,
    provider: Arc<rust_dix::ServiceProvider>,
    identity: Option<Arc<webx_core::auth::IdentityUser>>,
    limits: WebSocketLimits,
}

#[async_trait]
impl IUpgradeSession for WebSocketSession {
    async fn run(&self, transport: UpgradedTransport, meta: ConnectionMeta) -> Result<()> {
        let socket = WebSocketStream::from_raw_socket(
            transport,
            Role::Server,
            Some(WebSocketConfig::default()),
        )
        .await;
        let (sink, stream) = socket.split();
        // One DI scope per connection: connection-scoped services are shared by
        // every message on this connection and disposed when the session ends.
        let scope = Arc::new(ConnectionScope::new(self.provider.scope()));
        let context = Arc::new(ConnectionContext::new(meta, self.identity.clone(), scope));
        let limits = Arc::new(self.limits.clone());
        let stats = Arc::new(AtomicConnStats::default());
        // Process-wide metrics accumulator (registered by the host).
        let realtime_metrics = context.resolve::<RealtimeMetrics>();
        if let Some(metrics) = &realtime_metrics {
            metrics.record_connection_opened();
        }
        // Bounded outbound queue: a slow client can never block the sender.
        let (sender, receiver) = mpsc::channel(limits.outbound_queue);
        let writer = tokio::spawn(write_loop(sink, receiver));
        let connection = Arc::new(WebSocketConnection {
            context: Arc::clone(&context),
            sender,
            stream: Mutex::new(stream),
            limits: Arc::clone(&limits),
            stats: Arc::clone(&stats),
            metrics: realtime_metrics.clone(),
            close_recorded: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            window: std::sync::Mutex::new(InboundWindow::new()),
        });
        // Register for fan-out/presence; the host registers the in-memory
        // registry by default when WebSocket routes exist.
        let registry = context.resolve::<dyn webx_core::realtime::IConnectionRegistry>();
        if let Some(registry) = &registry {
            registry.register(Arc::clone(&connection) as Arc<dyn IConnection>);
        }
        let result = self
            .handler
            .on_connect(Arc::clone(&connection) as Arc<dyn IConnection>)
            .await;
        if let Some(registry) = &registry {
            registry.unregister(connection.id());
        }
        // Best-effort graceful close; dropping the last outbound sender lets the
        // writer flush and close the socket.
        let _ = connection.close(1000, "server closed").await;
        drop(connection);
        let _ = writer.await;
        if let Some(metrics) = &realtime_metrics {
            metrics.record_connection_closed();
        }
        result
    }
}

/// Per-connection counters behind [`ConnectionStats`].
#[derive(Default)]
struct AtomicConnStats {
    messages_in: AtomicU64,
    messages_out: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    dropped_outbound: AtomicU64,
    rate_limited: AtomicU64,
}

impl AtomicConnStats {
    fn snapshot(&self) -> ConnectionStats {
        ConnectionStats {
            messages_in: self.messages_in.load(Ordering::Relaxed),
            messages_out: self.messages_out.load(Ordering::Relaxed),
            bytes_in: self.bytes_in.load(Ordering::Relaxed),
            bytes_out: self.bytes_out.load(Ordering::Relaxed),
            dropped_outbound: self.dropped_outbound.load(Ordering::Relaxed),
            rate_limited: self.rate_limited.load(Ordering::Relaxed),
        }
    }
}

/// One-second sliding window for inbound rate limiting.
struct InboundWindow {
    started: std::time::Instant,
    messages: u32,
    bytes: u64,
}

impl InboundWindow {
    fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            messages: 0,
            bytes: 0,
        }
    }

    /// Count one inbound frame and return whether it is within the limits.
    fn allow(&mut self, bytes: u64, limits: &WebSocketLimits) -> bool {
        if self.started.elapsed() >= Duration::from_secs(1) {
            self.started = std::time::Instant::now();
            self.messages = 0;
            self.bytes = 0;
        }
        if limits.max_messages_per_second > 0
            && self.messages.saturating_add(1) > limits.max_messages_per_second
        {
            return false;
        }
        if limits.max_bytes_per_second > 0
            && self.bytes.saturating_add(bytes) > limits.max_bytes_per_second
        {
            return false;
        }
        self.messages += 1;
        self.bytes = self.bytes.saturating_add(bytes);
        true
    }
}

/// Drain the outbound queue into the socket, then close the socket.
async fn write_loop(mut sink: SessionSink, mut receiver: mpsc::Receiver<Message>) {
    while let Some(message) = receiver.recv().await {
        let is_close = matches!(message, Message::Close(_));
        if sink.send(message).await.is_err() {
            break;
        }
        if is_close {
            break;
        }
    }
    // Flush the close handshake / socket.
    let _ = sink.close().await;
}

fn payload_bytes(message: &Message) -> u64 {
    match message {
        Message::Text(text) => text.len() as u64,
        Message::Binary(bytes) => bytes.len() as u64,
        Message::Close(Some(frame)) => frame.reason.len() as u64,
        _ => 0,
    }
}

/// Queue one outbound frame, applying the backpressure policy when full.
fn enqueue_frame(
    sender: &mpsc::Sender<Message>,
    limits: &WebSocketLimits,
    stats: &AtomicConnStats,
    metrics: Option<&RealtimeMetrics>,
    closed: bool,
    message: Message,
) -> Result<()> {
    if closed {
        return Err(Error::Http("websocket connection is closed".to_string()));
    }
    let bytes = payload_bytes(&message);
    match sender.try_send(message) {
        Ok(()) => {
            stats.messages_out.fetch_add(1, Ordering::Relaxed);
            stats.bytes_out.fetch_add(bytes, Ordering::Relaxed);
            if let Some(metrics) = metrics {
                metrics.record_message_out(bytes);
            }
            Ok(())
        }
        Err(mpsc::error::TrySendError::Full(_)) => match limits.on_queue_full {
            BackpressurePolicy::Drop => {
                stats.dropped_outbound.fetch_add(1, Ordering::Relaxed);
                if let Some(metrics) = metrics {
                    metrics.record_dropped_outbound();
                }
                Ok(())
            }
            BackpressurePolicy::Disconnect => Err(Error::Http(
                "websocket backpressure: outbound queue full".to_string(),
            )),
        },
        Err(mpsc::error::TrySendError::Closed(_)) => {
            Err(Error::Http("websocket writer stopped".to_string()))
        }
    }
}

/// [`IConnection`] over a framed WebSocket stream with bounded backpressure.
pub(crate) struct WebSocketConnection {
    context: Arc<ConnectionContext>,
    sender: mpsc::Sender<Message>,
    stream: Mutex<SessionStream>,
    limits: Arc<WebSocketLimits>,
    stats: Arc<AtomicConnStats>,
    metrics: Option<Arc<RealtimeMetrics>>,
    close_recorded: AtomicBool,
    closed: AtomicBool,
    window: std::sync::Mutex<InboundWindow>,
}

impl WebSocketConnection {
    fn mark_closed(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    /// Count one session close, exactly once per connection.
    fn record_close_once(&self, code: u16) {
        if !self.close_recorded.swap(true, Ordering::SeqCst) {
            if let Some(metrics) = &self.metrics {
                metrics.record_close(code);
            }
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Queue one outbound frame, applying the backpressure policy when full.
    fn enqueue(&self, message: Message) -> Result<()> {
        enqueue_frame(
            &self.sender,
            &self.limits,
            &self.stats,
            self.metrics.as_deref(),
            self.is_closed(),
            message,
        )
    }

    fn send_error_frame(&self, code: &str, message: &str) {
        let frame = serde_json::json!({
            "error": { "code": code, "message": message, "retryable": false }
        });
        let _ = self.enqueue(Message::Text(frame.to_string().into()));
    }

    /// Apply the inbound rate limit; on violation queue an error frame and a
    /// close and report `false`.
    fn accept_inbound(&self, bytes: u64) -> bool {
        let mut window = self
            .window
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if window.allow(bytes, &self.limits) {
            return true;
        }
        drop(window);
        self.stats.rate_limited.fetch_add(1, Ordering::Relaxed);
        if let Some(metrics) = &self.metrics {
            metrics.record_rate_limited();
        }
        self.send_error_frame("rate_limited", "message rate limit exceeded");
        let _ = self.enqueue(Message::Close(Some(CloseFrame {
            code: CloseCode::from(1008),
            reason: "rate limit exceeded".into(),
        })));
        self.record_close_once(1008);
        self.mark_closed();
        false
    }
}

#[async_trait]
impl IConnection for WebSocketConnection {
    fn meta(&self) -> &ConnectionMeta {
        self.context.meta()
    }

    fn context(&self) -> &ConnectionContext {
        &self.context
    }

    fn stats(&self) -> ConnectionStats {
        self.stats.snapshot()
    }

    async fn recv(&self) -> Option<Result<RealtimeMessage>> {
        let mut stream = self.stream.lock().await;
        loop {
            if self.is_closed() {
                return None;
            }
            let next = match self.limits.idle_timeout {
                Some(timeout) => match tokio::time::timeout(timeout, stream.next()).await {
                    Ok(next) => next,
                    Err(_) => {
                        self.mark_closed();
                        let _ = self.enqueue(Message::Close(Some(CloseFrame {
                            code: CloseCode::from(1001),
                            reason: "idle timeout".into(),
                        })));
                        self.record_close_once(1001);
                        return None;
                    }
                },
                None => stream.next().await,
            };
            let message = match next? {
                Ok(message) => message,
                Err(error) => {
                    return Some(Err(Error::Internal(format!(
                        "websocket receive failed: {error}"
                    ))))
                }
            };
            match message {
                Message::Text(text) => {
                    let bytes = text.len() as u64;
                    if !self.accept_inbound(bytes) {
                        return None;
                    }
                    if bytes > self.limits.max_frame_bytes as u64 {
                        self.send_error_frame(
                            "frame_too_large",
                            &format!("frame exceeds {} bytes", self.limits.max_frame_bytes),
                        );
                        continue;
                    }
                    self.stats.messages_in.fetch_add(1, Ordering::Relaxed);
                    self.stats.bytes_in.fetch_add(bytes, Ordering::Relaxed);
                    if let Some(metrics) = &self.metrics {
                        metrics.record_message_in(bytes);
                    }
                    return Some(Ok(RealtimeMessage::Text(text.to_string())));
                }
                Message::Binary(bytes) => {
                    let len = bytes.len() as u64;
                    if !self.accept_inbound(len) {
                        return None;
                    }
                    if len > self.limits.max_frame_bytes as u64 {
                        self.send_error_frame(
                            "frame_too_large",
                            &format!("frame exceeds {} bytes", self.limits.max_frame_bytes),
                        );
                        continue;
                    }
                    self.stats.messages_in.fetch_add(1, Ordering::Relaxed);
                    self.stats.bytes_in.fetch_add(len, Ordering::Relaxed);
                    if let Some(metrics) = &self.metrics {
                        metrics.record_message_in(len);
                    }
                    return Some(Ok(RealtimeMessage::Binary(bytes.to_vec())));
                }
                Message::Close(frame) => {
                    let (code, reason) = match frame {
                        Some(frame) => (u16::from(frame.code), frame.reason.to_string()),
                        None => (1005, String::new()),
                    };
                    return Some(Ok(RealtimeMessage::Close { code, reason }));
                }
                // Control frames are handled by tungstenite (pong replies are
                // queued automatically); application code does not see them.
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            }
        }
    }

    async fn send_text(&self, text: String) -> Result<()> {
        self.enqueue(Message::Text(text.into()))
    }

    async fn send_binary(&self, bytes: Vec<u8>) -> Result<()> {
        self.enqueue(Message::Binary(bytes.into()))
    }

    async fn close(&self, code: u16, reason: &str) -> Result<()> {
        let result = self.enqueue(Message::Close(Some(CloseFrame {
            code: CloseCode::from(code),
            reason: reason.to_string().into(),
        })));
        self.record_close_once(code);
        self.mark_closed();
        result
    }
}

/// Message-as-request handler: each JSON text frame is decoded into `Req`,
/// dispatched through the mediator (the same `IRequestHandler` HTTP endpoints
/// use, in the per-message DI scope), and the encoded `Rsp` sent back as a JSON
/// frame. See `HostBuilder::use_websocket_request`.
///
/// Message handlers can reach the connection through
/// [`RequestContext::connection`] (metadata, trace id, identity and the
/// connection-scoped resolver) while their own `#[inject]` dependencies come
/// from the per-message scope.
pub struct WebSocketRequestHandler<Req, Rsp> {
    _marker: PhantomData<fn() -> (Req, Rsp)>,
}

impl<Req, Rsp> WebSocketRequestHandler<Req, Rsp> {
    /// Create the handler for the `Req -> Rsp` pair.
    pub fn new() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<Req, Rsp> Default for WebSocketRequestHandler<Req, Rsp> {
    fn default() -> Self {
        Self::new()
    }
}

/// Send one structured error frame.
async fn send_error_frame(
    connection: &dyn IConnection,
    code: &str,
    message: &str,
    retryable: bool,
    field: Option<&str>,
) -> Result<()> {
    let mut error = serde_json::json!({
        "code": code,
        "message": message,
        "retryable": retryable,
    });
    if let Some(field) = field {
        error["field"] = serde_json::Value::String(field.to_string());
    }
    let frame = serde_json::json!({ "error": error });
    connection.send_text(frame.to_string()).await
}

#[async_trait]
impl<Req, Rsp> IWebSocketHandler for WebSocketRequestHandler<Req, Rsp>
where
    Req: IRequest<Rsp> + DeserializeOwned + Send + 'static,
    Rsp: Serialize + Send + 'static,
{
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()> {
        let mediator = connection.context().resolve::<Mediator>().ok_or_else(|| {
            Error::Internal(
                "WebSocket request handler needs the mediator; call add_mediator() \
                 (the host registers it by default)"
                    .to_string(),
            )
        })?;
        let identity = connection.context().identity().map(Arc::clone);
        let trace_id: Option<Arc<str>> = connection.context().trace_id().map(Arc::from);
        let context = Arc::new(connection.context().clone());
        let realtime_metrics = connection.context().resolve::<RealtimeMetrics>();

        while let Some(message) = connection.recv().await {
            let text = match message? {
                RealtimeMessage::Text(text) => text,
                RealtimeMessage::Close { .. } => break,
                RealtimeMessage::Binary(_) => continue,
            };
            let mut deserializer = serde_json::Deserializer::from_str(&text);
            let request: Req = match serde_path_to_error::deserialize(&mut deserializer) {
                Ok(request) => request,
                Err(error) => {
                    let path = error.path().to_string();
                    let inner = error.into_inner();
                    // Syntax/EOF errors are protocol errors; data errors are the
                    // frame analogue of a 400 binding failure and carry the field
                    // path (`text`, `items[2].id`, ...).
                    let (code, message, field) =
                        if inner.is_syntax() || inner.is_eof() || inner.is_io() {
                            (
                                "invalid_message",
                                format!("frame is not valid JSON: {inner}"),
                                None,
                            )
                        } else {
                            // `path` is "." when serde reports the error at the
                            // struct root (for example a missing field, which has
                            // no path segment); only attach a real field path.
                            let field = (path != ".").then_some(path.as_str());
                            (
                                "validation_failed",
                                format!("field `{path}`: {inner}"),
                                field,
                            )
                        };
                    send_error_frame(connection.as_ref(), code, &message, false, field).await?;
                    continue;
                }
            };
            // The connection context is in scope for the whole dispatch, so the
            // handler can resolve connection-scoped services; the mediator
            // creates (and disposes) the per-message DI scope around the handler.
            let started = std::time::Instant::now();
            let result = RequestContext::run_in_connection(
                identity.clone(),
                trace_id.clone(),
                Arc::clone(&context),
                IMediator::send(mediator.as_ref(), request),
            )
            .await;
            if let Some(metrics) = &realtime_metrics {
                metrics.record_dispatch_latency(started.elapsed());
            }
            match result {
                Ok(reply) => {
                    let body = serde_json::to_string(&reply).map_err(|error| {
                        Error::Internal(format!("reply encode failed: {error}"))
                    })?;
                    connection.send_text(body).await?;
                }
                Err(error) => {
                    send_error_frame(
                        connection.as_ref(),
                        error.code().as_ref(),
                        error.public_detail().unwrap_or("request failed"),
                        error.retryable(),
                        None,
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webx_core::realtime::BackpressurePolicy;

    fn queue_limits(policy: BackpressurePolicy) -> WebSocketLimits {
        WebSocketLimits::default().with_outbound_queue(1, policy)
    }

    #[test]
    fn full_outbound_queue_drops_or_errors_per_policy() {
        let (sender, _receiver) = mpsc::channel::<Message>(1);
        let stats = AtomicConnStats::default();
        sender
            .try_send(Message::Text("taken".into()))
            .expect("fill queue");

        // Drop: the frame is discarded (counted) and send still succeeds.
        enqueue_frame(
            &sender,
            &queue_limits(BackpressurePolicy::Drop),
            &stats,
            None,
            false,
            Message::Text("x".into()),
        )
        .expect("drop policy keeps the caller alive");
        assert_eq!(stats.snapshot().dropped_outbound, 1);

        // Disconnect: the caller sees the backpressure error.
        let error = enqueue_frame(
            &sender,
            &queue_limits(BackpressurePolicy::Disconnect),
            &stats,
            None,
            false,
            Message::Text("x".into()),
        )
        .expect_err("disconnect policy must fail the send");
        assert!(error.to_string().contains("backpressure"));

        // A closed connection never queues.
        let error = enqueue_frame(
            &sender,
            &queue_limits(BackpressurePolicy::Drop),
            &stats,
            None,
            true,
            Message::Text("x".into()),
        )
        .expect_err("closed connection must reject sends");
        assert!(error.to_string().contains("closed"));
    }

    #[test]
    fn inbound_window_enforces_message_and_byte_rates() {
        let per_message = WebSocketLimits::default().with_max_messages_per_second(2);
        let mut window = InboundWindow::new();
        assert!(window.allow(1, &per_message));
        assert!(window.allow(1, &per_message));
        assert!(!window.allow(1, &per_message));

        let per_byte = WebSocketLimits::default().with_max_bytes_per_second(10);
        let mut window = InboundWindow::new();
        assert!(window.allow(6, &per_byte));
        assert!(!window.allow(6, &per_byte));
        assert!(window.allow(4, &per_byte));

        let unlimited = WebSocketLimits::default();
        let mut window = InboundWindow::new();
        for _ in 0..10_000 {
            assert!(window.allow(1024, &unlimited));
        }
    }
}
