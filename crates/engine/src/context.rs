//! `EngineContext` — the per-request request/response state of the 2.0 engine.
//!
//! The context holds hyper's request `Parts` (method, URI and headers moved out
//! of the request, never rebuilt into `String`s), the still-live request body,
//! the matched route and the outgoing response. It implements the
//! `webx_core` HTTP traits, so the binder, middleware and endpoint adapters
//! keep working unchanged.
//!
//! Zero-copy decisions:
//!
//! * [`IHttpRequest::method`]/[`IHttpRequest::path`] borrow from hyper's
//!   `Method`/`Uri`;
//! * headers are the original `HeaderMap` (case-insensitive, multi-valued);
//! * the query string is parsed lazily, on the first `query()`/`query_all()`;
//! * the route pattern is an `Arc<str>`; the `route_pattern_mut()` compatibility
//!   view materializes a `String` only when someone asks for it;
//! * route parameters are collected only when the router reports some;
//! * `body_bytes_shared()` hands out `Bytes`, with a single-frame fast path, and
//!   [`body_bytes`](IHttpRequest::body_bytes) is the compatible copy.
//!
//! The body is a small state machine (`Pending` -> `Buffered`/`Form`) and every
//! body method takes `&mut self`; there is no `Mutex` around it.

use bytes::Bytes;
use hyper::body::{Body, Frame, Incoming};
use hyper::header::{HeaderMap, HeaderName};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Instant;

use tracing::Instrument;
use webx_core::auth::IClaims;
use webx_core::dispatch_runtime::DispatchRuntime;
use webx_core::error::{Error, Result};
use webx_core::form::MultipartForm;
use webx_core::http::{
    IClaimsExt, IHttpContext, IHttpRequest, IHttpResponse, RequestBodyStream, ResponseBody,
};

use crate::body::{self, BodyLimits};
use crate::response::{self, EndpointResult, RespBody, ResponseBodyState, ResponseParts};
use crate::transport::{TransportBodyFrame, TransportRequestBody, TransportRequestBodyStream};

/// Concrete implementation of `IHttpContext` for one engine request.
pub struct EngineContext {
    req: EngineRequest,
    resp: EngineResponse,
    /// When the context was created (effectively "request accepted"), used by
    /// observability middleware for latency histograms.
    started_at: std::time::Instant,
    /// Authentication claims set by authentication middleware.
    claims: Option<Box<dyn IClaims>>,
    /// DI handles (provider + handler registry) for endpoint dispatch.
    dispatch_runtime: Option<Arc<DispatchRuntime>>,
    /// Trace id assigned by the tracing middleware; the same value the
    /// `x-trace-id` response header carries and handlers read through
    /// `RequestContext::trace_id()`.
    trace_id: Option<String>,
    /// Client information resolved by the host's `ClientInfo` middleware.
    client_info: Option<webx_core::client_info::ClientInfo>,
    /// Pending protocol upgrade (WebSocket), when the client asked for one.
    ///
    /// Hyper `OnUpgrade` values are wrapped into this neutral future by
    /// [`EngineContext::with_upgrade`]; a non-hyper transport supplies it on
    /// [`crate::transport::TransportRequest::upgrade`]. [`EngineContext::finalize`]
    /// spawns the session runner once the response is committed.
    upgrade: Option<crate::transport::TransportUpgrade>,
    /// Upgraded session installed by middleware/handler (`set_upgrade_session`).
    upgrade_session: Option<(
        Arc<dyn webx_core::realtime::IUpgradeSession>,
        webx_core::realtime::ConnectionMeta,
    )>,
    /// Detached upgrade-session counter owner (engine `Shared`).
    ///
    /// Workers use it during graceful drain so a spawned session task can flush
    /// its close frame before the worker runtime is dropped.
    upgrade_shared: Option<Arc<crate::shared::Shared>>,
}

impl EngineContext {
    /// Create a context around `req`.
    ///
    /// No I/O happens here: the body stays a live stream until the endpoint
    /// asks for it. `peer` is the client address captured by the accept loop
    /// (`None` when the host cannot report it).
    pub fn new(
        req: hyper::Request<Incoming>,
        limits: Arc<BodyLimits>,
        peer: Option<SocketAddr>,
        dispatch: Arc<DispatchRuntime>,
    ) -> Self {
        let (parts, body) = req.into_parts();
        let request = crate::hyper_compat::request(parts, body, peer);
        Self::from_transport(request, limits, Some(dispatch))
    }

    /// Create a context from a neutral transport request.
    ///
    /// This is the production constructor for non-hyper transports.
    /// The hyper path funnels through it as well, via
    /// [`EngineContext::new`], so both transports share one body state machine.
    ///
    /// The verified client-certificate fingerprint carried by the transport is
    /// applied here (strip any client-supplied header, then write the verified
    /// value), so the "TLS terminator is the only writer" contract holds for any
    /// transport — not just for the engine's own dispatch entry point.
    pub fn from_transport(
        request: crate::transport::TransportRequest,
        limits: Arc<BodyLimits>,
        dispatch: Option<Arc<DispatchRuntime>>,
    ) -> Self {
        let crate::transport::TransportRequest {
            mut parts,
            body,
            peer,
            upgrade,
            client_cert_fingerprint,
        } = request;
        // The TLS terminator is the only source of truth for the client
        // certificate: never let a plain-HTTP or upstream request spoof it.
        parts
            .headers
            .remove(webx_core::http::CLIENT_CERT_FINGERPRINT_HEADER);
        if let Some(fingerprint) = client_cert_fingerprint.as_deref() {
            if let Ok(value) = http::header::HeaderValue::from_str(fingerprint) {
                parts.headers.insert(
                    http::header::HeaderName::from_static(
                        webx_core::http::CLIENT_CERT_FINGERPRINT_HEADER,
                    ),
                    value,
                );
            }
        }
        let body = match body {
            TransportRequestBody::Empty => BodyState::Empty,
            TransportRequestBody::Buffered(bytes) => BodyState::Buffered(bytes),
            TransportRequestBody::Streaming { frames, exact_len } => {
                BodyState::Pending(Some(PendingBody { frames, exact_len }))
            }
        };
        let mut context = Self::from_request(EngineRequest::new(parts, body, limits, peer));
        context.dispatch_runtime = dispatch;
        context.upgrade = upgrade;
        context
    }

    /// Create a context from moved hyper request parts and an already-buffered
    /// body.
    ///
    /// `hyper::body::Incoming` cannot be constructed outside a live connection,
    /// so this is the in-process/test constructor (the host adapter's embedded
    /// tests use it too). The early oversized-`Content-Length` check is the
    /// same one [`EngineContext::new`] performs.
    pub fn from_parts(
        parts: http::request::Parts,
        body: Bytes,
        limits: Arc<BodyLimits>,
        peer: Option<SocketAddr>,
        dispatch: Option<Arc<DispatchRuntime>>,
    ) -> Self {
        let mut context = Self::from_request(EngineRequest::new(
            parts,
            BodyState::Buffered(body),
            limits,
            peer,
        ));
        context.dispatch_runtime = dispatch;
        context
    }

    fn from_request(req: EngineRequest) -> Self {
        Self {
            req,
            resp: EngineResponse::new(200),
            started_at: std::time::Instant::now(),
            claims: None,
            dispatch_runtime: None,
            trace_id: None,
            client_info: None,
            upgrade: None,
            upgrade_session: None,
            upgrade_shared: None,
        }
    }

    /// Attach the connection's pending protocol upgrade.
    pub fn with_upgrade(mut self, upgrade: hyper::upgrade::OnUpgrade) -> Self {
        self.upgrade = Some(crate::hyper_compat::upgrade(upgrade));
        self
    }

    /// Track detached upgrade-session tasks for graceful drain.
    pub(crate) fn track_upgrade_sessions(&mut self, shared: Arc<crate::shared::Shared>) {
        self.upgrade_shared = Some(shared);
    }

    /// Record the route matched by the router.
    ///
    /// `pattern` is the matched template (e.g. `/api/users/{id}`) and `params`
    /// the values captured from it. Static routes pass an empty `params`, and
    /// the compatibility `route_params()` map stays empty and allocation-free.
    pub fn set_route(&mut self, pattern: Arc<str>, params: Vec<(&'static str, String)>) {
        self.req.set_route(pattern, params);
    }

    /// Strip representation-specific headers before replacing the response with
    /// an error. Used by the engine's single error writer.
    pub(crate) fn strip_entity_headers(&mut self) {
        response::strip_entity_headers(&mut self.resp.headers);
    }

    /// Write the result of one endpoint dispatch into the response.
    ///
    /// * [`EndpointResult::Empty`] — `204 No Content` with no body;
    /// * [`EndpointResult::Json`] — the serialized bytes with an explicit
    ///   status and `content-type: application/json`;
    /// * [`EndpointResult::Action`] — the action result executes itself (files,
    ///   SSE, custom headers/status/body).
    pub async fn write_endpoint_result(&mut self, result: EndpointResult) -> Result<()> {
        let kind = match &result {
            EndpointResult::Empty => "empty",
            EndpointResult::Json { .. } => "json",
            EndpointResult::Action(_) => "action",
        };
        let span = tracing::debug_span!(
            target: "webx.http",
            "http.response_write",
            kind,
            status = tracing::field::Empty,
        );
        let outcome = async {
            match result {
                EndpointResult::Empty => {
                    self.resp.status = 204;
                    self.resp.body = ResponseBodyState::None;
                }
                EndpointResult::Json { status, body } => {
                    self.resp.status = status;
                    response::insert_static_header(
                        &mut self.resp.headers,
                        "content-type",
                        "application/json",
                    );
                    self.resp.body = ResponseBodyState::Bytes(body);
                }
                EndpointResult::Action(action) => action.execute(self).await?,
            }
            Ok(())
        }
        .instrument(span.clone())
        .await;
        span.record("status", u64::from(self.resp.status));
        outcome
    }

    /// Finish the request and produce the hyper response.
    ///
    /// Security headers, request ids and cache policy are middleware concerns;
    /// this applies only status, headers, content type and the body (including
    /// HEAD suppression and file/range semantics).
    ///
    /// An SSE response is a long connection: this is where `connection.opened`
    /// is emitted and the body learns to emit `connection.closed` when hyper
    /// finishes or drops it (see `docs/design/observability-contract.md`).
    pub async fn finalize(self) -> hyper::Response<RespBody> {
        let Self {
            mut req,
            resp,
            upgrade,
            upgrade_session,
            upgrade_shared,
            trace_id,
            ..
        } = self;
        spawn_upgrade_session(upgrade, upgrade_session, upgrade_shared);
        let response = resp.finalize(&mut req).await;
        observe_sse_response(response, trace_id, req.path())
    }

    /// Finish the request and produce a neutral transport response.
    ///
    /// Reuses [`Self::finalize`] (security headers, body suppression,
    /// file/range assembly, SSE lifecycle observation) and only changes the
    /// response representation, so a non-hyper transport sees exactly the
    /// bytes the hyper path would have written.
    pub async fn finalize_transport(self) -> crate::transport::TransportResponse {
        crate::hyper_compat::response(self.finalize().await)
    }
}

/// Whether `response` is a `text/event-stream` response (a long connection).
fn is_sse_response(response: &hyper::Response<RespBody>) -> bool {
    response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"))
}

/// Emit the SSE connection lifecycle events around an assembled response.
///
/// SSE has no protocol upgrade, so the engine owns the connection observation:
/// `connection.opened` fires here, right before hyper starts streaming the
/// body, and the wrapped body emits `connection.closed` exactly once when the
/// stream ends, errors, or is dropped (client disconnect / server abort). This
/// costs one header lookup per request and one boxed body per SSE connection —
/// regular responses are returned untouched.
fn observe_sse_response(
    response: hyper::Response<RespBody>,
    trace_id: Option<String>,
    path: &str,
) -> hyper::Response<RespBody> {
    if !is_sse_response(&response) {
        return response;
    }
    let connection_id = webx_core::realtime::ConnectionId::next();
    tracing::info!(
        target: "webx.connection",
        event = "connection.opened",
        transport = "sse",
        connection_id = %connection_id,
        trace_id = trace_id.as_deref().unwrap_or(""),
        path,
        "connection.opened"
    );
    let (parts, body) = response.into_parts();
    let body = http_body_util::BodyExt::boxed_unsync(SseLifecycleBody {
        inner: body,
        started: Instant::now(),
        connection_id,
        trace_id,
        finished: false,
    });
    hyper::Response::from_parts(parts, body)
}

/// SSE response body wrapper that reports `connection.closed` once.
struct SseLifecycleBody {
    inner: RespBody,
    started: Instant,
    connection_id: webx_core::realtime::ConnectionId,
    trace_id: Option<String>,
    finished: bool,
}

impl SseLifecycleBody {
    fn finish(&mut self, reason: &'static str) {
        if self.finished {
            return;
        }
        self.finished = true;
        let duration_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        tracing::info!(
            target: "webx.connection",
            event = "connection.closed",
            transport = "sse",
            connection_id = %self.connection_id,
            trace_id = self.trace_id.as_deref().unwrap_or(""),
            reason,
            duration_ms,
            "connection.closed"
        );
    }
}

impl Body for SseLifecycleBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(None) => {
                this.finish("completed");
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(error))) => {
                this.finish("failed");
                Poll::Ready(Some(Err(error)))
            }
            other => other,
        }
    }
}

impl Drop for SseLifecycleBody {
    fn drop(&mut self) {
        // Dropped before the framed stream finished: the client disconnected or
        // the connection was aborted while the response body was still live.
        self.finish("client_disconnected");
    }
}

/// Drive a committed protocol upgrade on the worker runtime.
///
/// The session future owns the upgraded transport; hyper's upgradeable
/// connection future stays pending until the upgraded IO closes, so the
/// connection remains counted by admission, metrics and graceful drain while
/// the session runs.
fn spawn_upgrade_session(
    upgrade: Option<crate::transport::TransportUpgrade>,
    session: Option<(
        Arc<dyn webx_core::realtime::IUpgradeSession>,
        webx_core::realtime::ConnectionMeta,
    )>,
    tracker: Option<Arc<crate::shared::Shared>>,
) {
    let (Some(upgrade), Some((session, meta))) = (upgrade, session) else {
        return;
    };
    // Count the session on every exit path (normal return, error, task abort).
    let guard = tracker.map(|shared| shared.admission.track_upgrade_session());
    tokio::spawn(async move {
        let _guard = guard;
        let transport_kind = meta.transport;
        let connection_id = meta.id;
        let trace_id = meta.trace_id.clone();
        match upgrade.io.await {
            Ok(transport) => {
                let started = Instant::now();
                tracing::info!(
                    target: "webx.connection",
                    event = "connection.opened",
                    transport = transport_kind.as_str(),
                    connection_id = %connection_id,
                    trace_id = trace_id.as_deref().unwrap_or(""),
                    "connection.opened"
                );
                let result = session.run(transport, meta).await;
                let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                match &result {
                    Ok(()) => tracing::info!(
                        target: "webx.connection",
                        event = "connection.closed",
                        transport = transport_kind.as_str(),
                        connection_id = %connection_id,
                        trace_id = trace_id.as_deref().unwrap_or(""),
                        reason = "completed",
                        duration_ms,
                        "connection.closed"
                    ),
                    Err(error) => tracing::warn!(
                        target: "webx.connection",
                        event = "connection.closed",
                        transport = transport_kind.as_str(),
                        connection_id = %connection_id,
                        trace_id = trace_id.as_deref().unwrap_or(""),
                        reason = "error",
                        duration_ms,
                        error = %error,
                        "connection.closed"
                    ),
                }
            }
            Err(error) => tracing::warn!(%error, "engine: protocol upgrade failed"),
        }
    });
}

impl IClaimsExt for EngineContext {
    fn set_claims(&mut self, claims: Box<dyn IClaims>) {
        self.claims = Some(claims);
    }

    fn claims(&self) -> Option<&dyn IClaims> {
        self.claims.as_deref()
    }
}

impl IHttpContext for EngineContext {
    fn request(&self) -> &dyn IHttpRequest {
        &self.req
    }

    fn request_mut(&mut self) -> &mut dyn IHttpRequest {
        &mut self.req
    }

    fn response(&self) -> &dyn IHttpResponse {
        &self.resp
    }

    fn response_mut(&mut self) -> &mut dyn IHttpResponse {
        &mut self.resp
    }

    fn take_early_error(&mut self) -> Option<Error> {
        // Declared-length validation now happens after route/limit resolution
        // and inside the body accessors, so there is no construction-time
        // error to report here.
        None
    }

    fn dispatch_runtime(&self) -> Option<&Arc<DispatchRuntime>> {
        self.dispatch_runtime.as_ref()
    }

    fn trace_id(&self) -> Option<&str> {
        self.trace_id.as_deref()
    }

    fn set_trace_id(&mut self, trace_id: &str) {
        self.trace_id = Some(trace_id.to_string());
    }

    fn elapsed(&self) -> Option<std::time::Duration> {
        Some(self.started_at.elapsed())
    }

    fn set_client_info(&mut self, info: webx_core::client_info::ClientInfo) {
        self.client_info = Some(info);
    }

    fn client_info(&self) -> webx_core::client_info::ClientInfo {
        self.client_info.clone().unwrap_or_else(|| {
            webx_core::client_info::ClientInfo::from_peer(self.req.remote_addr())
        })
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn set_upgrade_session(
        &mut self,
        session: Arc<dyn webx_core::realtime::IUpgradeSession>,
        meta: webx_core::realtime::ConnectionMeta,
    ) {
        self.upgrade_session = Some((session, meta));
    }
}

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// State of the request body.
enum BodyState {
    /// Known to carry no body; reading it yields zero chunks, not one empty one.
    Empty,
    /// Still streaming from the connection. `None` means it was already taken.
    Pending(Option<PendingBody>),
    /// Fully buffered, replayable. `Bytes` keeps a clone cheap instead of
    /// copying the whole body again.
    Buffered(Bytes),
    /// Parsed as a multipart form.
    Form(Arc<MultipartForm>),
}

/// A live neutral request body plus its declared length.
struct PendingBody {
    frames: TransportRequestBodyStream,
    exact_len: Option<u64>,
}

/// Concrete implementation of `IHttpRequest`.
pub struct EngineRequest {
    /// hyper's request parts: method, URI, headers and extensions, moved.
    parts: http::request::Parts,
    peer: Option<SocketAddr>,
    body: BodyState,
    /// Global body/upload limits shared with the engine.
    limits: Arc<BodyLimits>,
    /// Effective non-multipart body cap; `#[max_body_size]` overrides it per
    /// route through [`IHttpRequest::set_max_body_size`].
    body_limit: usize,
    /// Declared `Content-Length`, if any. Evaluated after routing so a route
    /// limit can lower *or raise* the global default.
    declared_length: Option<u64>,
    route_params: HashMap<String, String>,
    /// Matched route template, kept as an `Arc<str>` so static routes do not
    /// allocate a `String` per request.
    route_pattern: Option<Arc<str>>,
    /// Compatibility view for [`IHttpRequest::route_pattern_mut`]; `Some`
    /// once someone asked for mutable access.
    route_pattern_compat: Option<Option<String>>,
    /// Lazily parsed query-string views.
    query: OnceLock<QueryViews>,
}

impl EngineRequest {
    fn new(
        parts: http::request::Parts,
        body: BodyState,
        limits: Arc<BodyLimits>,
        peer: Option<SocketAddr>,
    ) -> Self {
        let body_limit = limits.body;
        let declared_length = parts
            .headers
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        Self {
            parts,
            peer,
            body,
            limits,
            body_limit,
            declared_length,
            route_params: HashMap::new(),
            route_pattern: None,
            route_pattern_compat: None,
            query: OnceLock::new(),
        }
    }

    fn set_route(&mut self, pattern: Arc<str>, params: Vec<(&'static str, String)>) {
        self.route_pattern = Some(pattern);
        self.route_pattern_compat = None;
        // An empty iterator collects into an empty map without allocating, so a
        // static route keeps the map untouched.
        self.route_params = params
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect();
    }

    fn query_views(&self) -> &QueryViews {
        self.query.get_or_init(|| match self.parts.uri.query() {
            Some(query) if !query.is_empty() => parse_query_string(query),
            _ => QueryViews::default(),
        })
    }

    fn ensure_declared_within(&self, limit: u64) -> Result<()> {
        match self.declared_length {
            Some(declared) if declared > limit => Err(Error::payload_too_large(format!(
                "request body exceeds the {limit}-byte limit"
            ))),
            _ => Ok(()),
        }
    }
}

#[async_trait::async_trait]
impl IHttpRequest for EngineRequest {
    fn method(&self) -> &str {
        self.parts.method.as_str()
    }

    fn path(&self) -> &str {
        self.parts.uri.path()
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.parts
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
    }

    fn header_all(&self, name: &str) -> Vec<&str> {
        self.parts
            .headers
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect()
    }

    fn header_pairs(&self) -> Box<dyn Iterator<Item = (&str, &str)> + '_> {
        Box::new(
            self.parts.headers.iter().filter_map(|(name, value)| {
                value.to_str().ok().map(|value| (name.as_str(), value))
            }),
        )
    }

    fn query(&self) -> &HashMap<String, String> {
        &self.query_views().flat
    }

    fn query_all(&self) -> Option<&HashMap<String, Vec<String>>> {
        Some(&self.query_views().multi)
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        self.peer
    }

    fn set_max_body_size(&mut self, limit: usize) {
        self.body_limit = limit;
    }

    fn validate_declared_body_limit(&mut self) -> Result<()> {
        self.ensure_declared_within(self.body_limit as u64)
    }

    fn route_params(&self) -> &HashMap<String, String> {
        &self.route_params
    }

    fn route_params_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.route_params
    }

    fn route_pattern(&self) -> Option<&str> {
        match &self.route_pattern_compat {
            Some(view) => view.as_deref(),
            None => self.route_pattern.as_deref(),
        }
    }

    fn route_pattern_mut(&mut self) -> &mut Option<String> {
        if self.route_pattern_compat.is_none() {
            self.route_pattern_compat = Some(self.route_pattern.as_deref().map(str::to_owned));
        }
        self.route_pattern_compat
            .as_mut()
            .expect("the compatibility view was initialised above")
    }

    async fn body_bytes(&mut self) -> Result<Vec<u8>> {
        Ok(self.body_bytes_shared().await?.to_vec())
    }

    async fn body_bytes_shared(&mut self) -> Result<Bytes> {
        // The body read is the I/O half of model binding; the pure in-memory
        // path/query mapping that follows is not a separate span.
        let span = tracing::debug_span!(
            target: "webx.http",
            "http.binding",
            kind = "bytes",
            limit = self.body_limit as u64,
            bytes = tracing::field::Empty,
        );
        let outcome = async {
            self.ensure_declared_within(self.body_limit as u64)?;
            match &self.body {
                BodyState::Empty => return Ok(Bytes::new()),
                BodyState::Buffered(bytes) => {
                    if bytes.len() > self.body_limit {
                        return Err(Error::payload_too_large(format!(
                            "request body exceeds the {}-byte limit",
                            self.body_limit
                        )));
                    }
                    return Ok(bytes.clone());
                }
                BodyState::Form(_) => {
                    return Err(Error::state(
                        "request body was already read as multipart/form-data".to_string(),
                    ))
                }
                BodyState::Pending(_) => {}
            }

            let pending = match &mut self.body {
                BodyState::Pending(slot) => slot.take(),
                _ => {
                    return Err(Error::state(
                        "request body has already been consumed".to_string(),
                    ))
                }
            };
            let Some(pending) = pending else {
                return Err(Error::state(
                    "request body has already been consumed".to_string(),
                ));
            };

            let bytes = body::read_body(pending.frames, pending.exact_len, self.body_limit).await?;
            self.body = BodyState::Buffered(bytes.clone());
            Ok(bytes)
        }
        .instrument(span.clone())
        .await;
        if let Ok(bytes) = &outcome {
            span.record("bytes", bytes.len() as u64);
        }
        outcome
    }

    async fn body_stream(&mut self) -> Result<RequestBodyStream> {
        self.ensure_declared_within(self.body_limit as u64)?;
        match &self.body {
            BodyState::Empty => {
                let stream: RequestBodyStream = Box::pin(futures_util::stream::empty());
                return Ok(stream);
            }
            BodyState::Form(_) => {
                return Err(Error::state(
                    "request body was already read as multipart/form-data".to_string(),
                ))
            }
            BodyState::Buffered(bytes) => {
                // A buffered body bypasses the streaming `read_body` limit
                // check (e.g. an already-decoded transport request), so apply
                // the same route/app cap here as `body_bytes_shared`.
                if bytes.len() > self.body_limit {
                    return Err(Error::payload_too_large(format!(
                        "request body exceeds the {}-byte limit",
                        self.body_limit
                    )));
                }
                let bytes = bytes.clone();
                return Ok(Box::pin(futures_util::stream::once(async move {
                    Ok::<Bytes, Error>(bytes)
                })));
            }
            BodyState::Pending(_) => {}
        }

        let pending = match &mut self.body {
            BodyState::Pending(slot) => slot.take(),
            _ => {
                return Err(Error::state(
                    "request body has already been consumed".to_string(),
                ))
            }
        };
        let Some(pending) = pending else {
            return Err(Error::state(
                "request body has already been consumed".to_string(),
            ));
        };

        Ok(body::stream_body(pending.frames, self.body_limit))
    }

    async fn body_text(&mut self) -> Result<String> {
        let bytes = self.body_bytes_shared().await?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| Error::validation("request body is not valid UTF-8".to_string()))
    }

    async fn multipart(&mut self) -> Result<Arc<MultipartForm>> {
        let span = tracing::debug_span!(
            target: "webx.http",
            "http.binding",
            kind = "multipart",
            limit = self.limits.multipart_body,
        );
        async {
            self.ensure_declared_within(self.limits.multipart_body)?;
            if let BodyState::Form(form) = &self.body {
                return Ok(Arc::clone(form));
            }

            let raw_content_type = self
                .parts
                .headers
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("");
            if !body::is_multipart_content_type(Some(raw_content_type)) {
                return Err(Error::unsupported_media_type(
                    "expected Content-Type: multipart/form-data".to_string(),
                ));
            }
            let boundary = body::multipart_boundary(raw_content_type).ok_or_else(|| {
                Error::validation(
                    "multipart/form-data request is missing its boundary parameter".to_string(),
                )
            })?;

            let frames: TransportRequestBodyStream = match &mut self.body {
                BodyState::Empty => Box::pin(futures_util::stream::empty()),
                // A body may already be buffered when it reaches this point: a
                // neutral transport (HTTP/3, in-process backends) delivers whole
                // bodies, and a middleware may have read it first. The bytes are
                // still a valid multipart payload, so replay them as one frame
                // instead of failing the upload with `415`.
                BodyState::Buffered(bytes) => {
                    // Whole-body transports (HTTP/3, in-process backends) have
                    // no Content-Length to check, so enforce the multipart cap
                    // on the buffered payload itself.
                    if bytes.len() as u64 > self.limits.multipart_body {
                        return Err(Error::payload_too_large(format!(
                            "multipart body exceeds the {}-byte limit",
                            self.limits.multipart_body
                        )));
                    }
                    let frame: std::result::Result<
                        TransportBodyFrame,
                        crate::transport::TransportBodyError,
                    > = Ok(TransportBodyFrame::Data(bytes.clone()));
                    Box::pin(futures_util::stream::iter(vec![frame]))
                }
                BodyState::Pending(slot) => {
                    let Some(pending) = slot.take() else {
                        return Err(Error::state(
                            "request body has already been consumed".to_string(),
                        ));
                    };
                    pending.frames
                }
                _ => {
                    return Err(Error::state(
                        "request body has already been consumed".to_string(),
                    ))
                }
            };

            let form = Arc::new(body::parse_multipart(frames, &boundary, &self.limits).await?);
            self.body = BodyState::Form(Arc::clone(&form));
            Ok(form)
        }
        .instrument(span)
        .await
    }
}

/// The two query-string views derived from the raw query.
#[derive(Default)]
struct QueryViews {
    /// Last value per key — the compatibility view of `IHttpRequest::query`.
    flat: HashMap<String, String>,
    /// Every value per key — what `Vec<T>` fields bind from.
    multi: HashMap<String, Vec<String>>,
}

/// Parse a query string like "key1=val1&key2=val2" into both views.
///
/// Repeated keys keep their order; the last value wins in the flat view; empty
/// pairs are ignored.
fn parse_query_string(query: &str) -> QueryViews {
    let mut views = QueryViews::default();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut parts = pair.splitn(2, '=');
        if let Some(key) = parts.next() {
            // `?verbose` is a present-empty query value in the HTTP binding
            // model (`?verbose=` is equivalent for scalar fields).
            let value = parts.next().unwrap_or("");
            let key = percent_decode(key);
            let value = percent_decode(value);
            views.flat.insert(key.clone(), value.clone());
            views.multi.entry(key).or_default().push(value);
        }
    }
    views
}

/// Decode `%XX` escapes and `+` into a UTF-8 string.
///
/// Escapes are decoded as bytes and then validated as UTF-8, so multi-byte
/// characters (`%E4%B8%AD` -> 中) survive.
fn percent_decode(s: &str) -> String {
    if !s.contains('%') && !s.contains('+') {
        return s.to_string();
    }

    let raw = s.as_bytes();
    let mut bytes = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        match raw[index] {
            b'%' if index + 2 < raw.len() => {
                let hi = (raw[index + 1] as char).to_digit(16);
                let lo = (raw[index + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    bytes.push((hi * 16 + lo) as u8);
                    index += 3;
                    continue;
                }
                // Invalid escape: preserve all three bytes rather than
                // silently dropping the characters that followed `%`.
                bytes.push(b'%');
                index += 1;
            }
            b'%' => {
                bytes.push(b'%');
                index += 1;
            }
            b'+' => {
                bytes.push(b' ');
                index += 1;
            }
            other => {
                bytes.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

/// Concrete implementation of `IHttpResponse`.
pub struct EngineResponse {
    status: u16,
    headers: HeaderMap,
    body: ResponseBodyState,
}

impl EngineResponse {
    fn new(status: u16) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: ResponseBodyState::None,
        }
    }

    async fn finalize(self, req: &mut dyn IHttpRequest) -> hyper::Response<RespBody> {
        ResponseParts {
            status: self.status,
            headers: self.headers,
            body: self.body,
        }
        .assemble(req)
        .await
    }
}

#[async_trait::async_trait]
impl IHttpResponse for EngineResponse {
    fn status(&self) -> u16 {
        self.status
    }

    fn has_body(&self) -> bool {
        !matches!(self.body, ResponseBodyState::None)
    }

    fn set_status(&mut self, code: u16) {
        self.status = code;
    }

    fn set_header(&mut self, key: &str, value: &str) {
        response::set_header(&mut self.headers, key, value);
    }

    fn set_header_owned(&mut self, key: &str, value: String) {
        response::set_header_owned(&mut self.headers, key, value);
    }

    fn set_header_static(&mut self, key: &'static str, value: &'static str) {
        response::insert_static_header(&mut self.headers, key, value);
    }

    fn append_header(&mut self, key: &str, value: &str) {
        response::append_header(&mut self.headers, key, value);
    }

    fn remove_header(&mut self, key: &str) {
        if let Ok(name) = HeaderName::from_bytes(key.as_bytes()) {
            self.headers.remove(name);
        }
    }

    async fn write_bytes(&mut self, data: Vec<u8>) -> Result<()> {
        self.body = ResponseBodyState::Bytes(Bytes::from(data));
        Ok(())
    }

    async fn write_bytes_shared(&mut self, data: Bytes) -> Result<()> {
        self.body = ResponseBodyState::Bytes(data);
        Ok(())
    }

    async fn write_text(&mut self, text: &str) -> Result<()> {
        self.body = ResponseBodyState::Bytes(Bytes::copy_from_slice(text.as_bytes()));
        Ok(())
    }

    async fn write_body(&mut self, body: ResponseBody) -> Result<()> {
        self.body = match body {
            ResponseBody::Bytes(bytes) => ResponseBodyState::Bytes(Bytes::from(bytes)),
            ResponseBody::File(file) => ResponseBodyState::File(file),
        };
        Ok(())
    }

    fn body_bytes(&self) -> Vec<u8> {
        match &self.body {
            ResponseBodyState::Bytes(bytes) => bytes.to_vec(),
            ResponseBodyState::None | ResponseBodyState::File(_) => Vec::new(),
        }
    }

    fn header(&self, key: &str) -> Option<&str> {
        self.headers.get(key).and_then(|value| value.to_str().ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    fn context(
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: &'static [u8],
    ) -> EngineContext {
        let mut builder = http::Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let parts = builder
            .body(())
            .expect("test request builds")
            .into_parts()
            .0;
        EngineContext::from_parts(
            parts,
            Bytes::from_static(body),
            Arc::new(BodyLimits::default()),
            None,
            None,
        )
    }

    async fn body_of(response: hyper::Response<RespBody>) -> Bytes {
        response
            .into_body()
            .collect()
            .await
            .expect("response body collects")
            .to_bytes()
    }

    #[tokio::test]
    async fn append_header_keeps_every_value() {
        let mut ctx = context("GET", "/", &[], b"");
        ctx.response_mut().append_header("set-cookie", "a=1");
        ctx.response_mut().append_header("set-cookie", "b=2");

        let response = ctx.finalize().await;
        let values: Vec<_> = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect();
        assert_eq!(values, ["a=1", "b=2"]);
    }

    #[tokio::test]
    async fn set_header_replaces_a_previous_value() {
        let mut ctx = context("GET", "/", &[], b"");
        ctx.response_mut().append_header("x-test", "one");
        ctx.response_mut().set_header("x-test", "two");

        let response = ctx.finalize().await;
        assert_eq!(
            response
                .headers()
                .get_all("x-test")
                .iter()
                .filter_map(|value| value.to_str().ok())
                .collect::<Vec<_>>(),
            ["two"]
        );
    }

    #[tokio::test]
    async fn set_header_owned_replaces_a_previous_value() {
        let mut ctx = context("GET", "/", &[], b"");
        ctx.response_mut().append_header("x-test", "one");
        ctx.response_mut()
            .set_header_owned("x-test", "two".to_string());

        let response = ctx.finalize().await;
        assert_eq!(
            response
                .headers()
                .get("x-test")
                .and_then(|value| value.to_str().ok()),
            Some("two")
        );
    }

    #[test]
    fn header_pairs_exposes_every_header_for_signature_verification() {
        let ctx = context(
            "POST",
            "/webhooks/stripe",
            &[
                ("stripe-signature", "t=1,v1=abc"),
                ("content-type", "application/json"),
            ],
            b"{}",
        );
        let pairs: Vec<(String, String)> = ctx
            .request()
            .header_pairs()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        assert!(pairs
            .iter()
            .any(|(name, value)| name == "stripe-signature" && value == "t=1,v1=abc"));
        assert!(pairs.iter().any(|(name, _)| name == "content-type"));
    }

    #[tokio::test]
    async fn head_keeps_content_length_and_drops_the_body() {
        let mut ctx = context("HEAD", "/report", &[], b"");
        ctx.response_mut()
            .set_header("content-type", "application/json");
        ctx.response_mut()
            .write_bytes(b"[1,2,3]".to_vec())
            .await
            .unwrap();

        let response = ctx.finalize().await;
        assert_eq!(response.headers()["content-length"], "7");
        assert_eq!(response.headers()["content-type"], "application/json");
        assert!(body_of(response).await.is_empty());
    }

    #[tokio::test]
    async fn get_keeps_the_body() {
        let mut ctx = context("GET", "/report", &[], b"");
        ctx.response_mut()
            .write_bytes(b"[1,2,3]".to_vec())
            .await
            .unwrap();

        let response = ctx.finalize().await;
        assert_eq!(&body_of(response).await[..], b"[1,2,3]");
    }

    #[tokio::test]
    async fn json_endpoint_result_sets_status_and_content_type() {
        let mut ctx = context("GET", "/", &[], b"");
        ctx.write_endpoint_result(EndpointResult::Json {
            status: 201,
            body: Bytes::from_static(b"{\"id\":1}"),
        })
        .await
        .unwrap();

        let response = ctx.finalize().await;
        assert_eq!(response.status(), 201);
        assert_eq!(response.headers()["content-type"], "application/json");
        assert_eq!(&body_of(response).await[..], b"{\"id\":1}");
    }

    #[tokio::test]
    async fn empty_endpoint_result_is_204_without_a_body() {
        let mut ctx = context("GET", "/", &[], b"");
        ctx.write_endpoint_result(EndpointResult::Empty)
            .await
            .unwrap();

        let response = ctx.finalize().await;
        assert_eq!(response.status(), 204);
        assert!(body_of(response).await.is_empty());
    }

    #[test]
    fn route_params_and_pattern_are_set_by_the_router() {
        let mut ctx = context("GET", "/users/7", &[], b"");
        // A static route has no parameters.
        assert!(ctx.request().route_params().is_empty());
        assert_eq!(ctx.request().route_pattern(), None);

        ctx.set_route(Arc::from("/users/{id}"), vec![("id", "7".to_string())]);
        assert_eq!(ctx.request().route_pattern(), Some("/users/{id}"));
        assert_eq!(
            ctx.request().route_params().get("id").map(String::as_str),
            Some("7")
        );

        // The mutable compatibility view materializes lazily and then wins.
        assert_eq!(
            ctx.request_mut().route_pattern_mut().as_deref(),
            Some("/users/{id}")
        );
        *ctx.request_mut().route_pattern_mut() = Some("/users/{id}/v2".to_string());
        assert_eq!(ctx.request().route_pattern(), Some("/users/{id}/v2"));
    }

    #[test]
    fn query_is_parsed_lazily_and_keeps_every_value() {
        let ctx = context(
            "GET",
            "/search?tag=a&tag=b&q=hello+world&p=%E4%B8%AD",
            &[],
            b"",
        );
        assert_eq!(
            ctx.request().query().get("tag").map(String::as_str),
            Some("b")
        );
        assert_eq!(
            ctx.request().query_all().unwrap().get("tag").unwrap(),
            &["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            ctx.request().query().get("q").map(String::as_str),
            Some("hello world")
        );
        assert_eq!(
            ctx.request().query().get("p").map(String::as_str),
            Some("中")
        );
    }

    #[test]
    fn oversized_declared_content_length_is_a_body_limit_error() {
        let mut ctx = context("POST", "/upload", &[("content-length", "999999999")], b"");
        // The default limit is 10 MiB; the check now runs after the route has
        // had a chance to apply `#[max_body_size]`.
        let error = ctx
            .request_mut()
            .validate_declared_body_limit()
            .expect_err("declared length exceeds the limit");
        assert_eq!(error.code(), "payload_too_large");
        // The response was never written eagerly.
        assert_eq!(ctx.response().status(), 200);
        assert!(ctx.take_early_error().is_none());
    }

    #[tokio::test]
    async fn multipart_is_only_parsed_from_a_multipart_request() {
        let mut ctx = context(
            "POST",
            "/upload",
            &[("content-type", "application/json")],
            b"",
        );
        let error = ctx
            .request_mut()
            .multipart()
            .await
            .expect_err("JSON is not multipart");
        assert_eq!(error.code(), "unsupported_media_type");
    }

    #[tokio::test]
    async fn multipart_parses_an_already_buffered_body() {
        // Neutral transports (HTTP/3, in-process backends) hand the engine a
        // whole body, so a multipart upload must bind from the buffered state
        // instead of failing with `415`.
        let mut ctx = context(
            "POST",
            "/upload",
            &[("content-type", "multipart/form-data; boundary=X-BOUNDARY")],
            b"--X-BOUNDARY\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nhello\r\n--X-BOUNDARY--\r\n",
        );
        let form = ctx
            .request_mut()
            .multipart()
            .await
            .expect("a buffered multipart body parses");
        assert_eq!(form.text("title"), Some("hello"));
    }

    #[test]
    fn from_transport_applies_the_verified_client_cert_fingerprint() {
        // A client-supplied header must never survive; only the value the
        // transport's TLS terminator verified is written.
        let request = http::Request::builder()
            .method("GET")
            .uri("/")
            .header(webx_core::http::CLIENT_CERT_FINGERPRINT_HEADER, "spoofed")
            .body(())
            .expect("test request builds");
        let (parts, ()) = request.into_parts();
        let transport_request = crate::transport::TransportRequest::new(parts, Bytes::new(), None)
            .with_client_cert_fingerprint("verified");
        let ctx =
            EngineContext::from_transport(transport_request, Arc::new(BodyLimits::default()), None);
        assert_eq!(
            ctx.request()
                .header(webx_core::http::CLIENT_CERT_FINGERPRINT_HEADER),
            Some("verified")
        );
    }
}
