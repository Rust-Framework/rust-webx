//! Pooled HTTP client, request builder and streaming response body.

use crate::error::HttpClientError;
use crate::sse::SseStream;
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures_util::stream::{Stream, StreamExt};
use http::header::{HeaderName, HeaderValue, USER_AGENT};
use http::{HeaderMap, Method, Request, Uri};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::connect::dns::{GaiResolver, Name};
use hyper_util::client::legacy::connect::proxy::Tunnel;
use hyper_util::client::legacy::connect::{Connect, HttpConnector};
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::rt::TokioExecutor;
use rustls::pki_types::{pem::PemObject, CertificateDer};
use rustls::RootCertStore;
use std::fmt;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::time::timeout;
use tower_service::Service;

static SPAN_SEQ: AtomicU64 = AtomicU64::new(1);

/// Headers that belong to one hop and must not cross a proxy boundary
/// (RFC 9110 §7.6.1 plus the legacy `Keep-Alive`/`Proxy-Connection` names).
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

type BodyError = Box<dyn std::error::Error + Send + Sync>;
type ClientBody = UnsyncBoxBody<Bytes, BodyError>;
type Https = HttpsConnector<HttpConnector<GuardedResolver>>;
type ProxyTunnel = Tunnel<Https>;

enum InnerClient {
    Direct(HyperClient<Https, ClientBody>),
    Proxied(HyperClient<ProxyTunnel, ClientBody>),
}

impl fmt::Debug for InnerClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Direct(_) => f.write_str("Direct(..)"),
            Self::Proxied(_) => f.write_str("Proxied(..)"),
        }
    }
}

/// A DNS resolver that re-checks every resolved address with the SSRF guard at
/// connect time, closing the DNS-rebinding gap (a public hostname whose DNS
/// answer flips to a private/metadata IP after the URI-string check).
///
/// When the guard is disabled it is a transparent pass-through to
/// [`GaiResolver`]. Literal IPs never reach here — hyper connects to them
/// directly, and they are already validated by `check_outbound_uri`.
#[derive(Clone)]
struct GuardedResolver {
    inner: GaiResolver,
    guard: bool,
}

impl GuardedResolver {
    fn new(guard: bool) -> Self {
        Self {
            inner: GaiResolver::new(),
            guard,
        }
    }
}

impl fmt::Debug for GuardedResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuardedResolver")
            .field("guard", &self.guard)
            .finish()
    }
}

/// Future produced by [`GuardedResolver`]: resolves, then filters blocked IPs.
struct GuardedResolveFuture {
    inner: <GaiResolver as Service<Name>>::Future,
    guard: bool,
}

impl Future for GuardedResolveFuture {
    type Output = Result<std::vec::IntoIter<SocketAddr>, io::Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Ready(Ok(addrs)) => {
                let all: Vec<SocketAddr> = addrs.collect();
                if !this.guard {
                    return Poll::Ready(Ok(all.into_iter()));
                }
                let allowed: Vec<SocketAddr> = all
                    .into_iter()
                    .filter(|addr| !is_blocked_ip(addr.ip()))
                    .collect();
                if allowed.is_empty() {
                    Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "all resolved addresses blocked by the outbound SSRF guard",
                    )))
                } else {
                    Poll::Ready(Ok(allowed.into_iter()))
                }
            }
        }
    }
}

impl Service<Name> for GuardedResolver {
    type Response = std::vec::IntoIter<SocketAddr>;
    type Error = io::Error;
    type Future = GuardedResolveFuture;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, name: Name) -> Self::Future {
        GuardedResolveFuture {
            inner: self.inner.call(name),
            guard: self.guard,
        }
    }
}

fn outgoing_traceparent() -> Option<String> {
    let trace_id = webx_core::request_context::RequestContext::trace_id()?;
    let trace_id = trace_id.as_ref();
    if trace_id.len() != 32 || !trace_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let span = SPAN_SEQ.fetch_add(1, Ordering::Relaxed);
    Some(format!(
        "00-{}-{span:016x}-01",
        trace_id.to_ascii_lowercase()
    ))
}

fn into_client_body(source: BodySource) -> ClientBody {
    match source {
        BodySource::Bytes(bytes) => {
            BodyExt::boxed_unsync(Full::new(bytes).map_err(|never| -> BodyError { match never {} }))
        }
        BodySource::Stream(stream) => {
            let frames = stream.map(|chunk| {
                chunk
                    .map(Frame::data)
                    .map_err(|error| -> BodyError { Box::new(error) })
            });
            BodyExt::boxed_unsync(StreamBody::new(frames))
        }
    }
}

/// A request body: in-memory bytes or a streaming source.
pub enum BodySource {
    /// Fully buffered body.
    Bytes(Bytes),
    /// Streamed chunks; errors abort the request.
    Stream(Pin<Box<dyn Stream<Item = Result<Bytes, HttpClientError>> + Send>>),
}

impl fmt::Debug for BodySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bytes(bytes) => f.debug_tuple("Bytes").field(&bytes.len()).finish(),
            Self::Stream(_) => f.write_str("Stream(..)"),
        }
    }
}

/// Configuration for one named HTTP client.
///
/// Every provider gets its own client (connection pool, base URL and
/// timeouts). Use [`HttpClientOptions::new`] plus struct update syntax:
///
/// ```ignore
/// let options = HttpClientOptions {
///     connect_timeout: Duration::from_secs(3),
///     request_timeout: Some(Duration::from_secs(120)),
///     pool_max_idle_per_host: 64,
///     ..HttpClientOptions::new("https://api.openai.com")
/// };
/// ```
#[derive(Debug, Clone)]
pub struct HttpClientOptions {
    /// Absolute base URL, e.g. `https://api.openai.com` or
    /// `https://api.openai.com/v1`.
    pub base_url: String,
    /// TCP/TLS connection establishment deadline.
    pub connect_timeout: Duration,
    /// Deadline for receiving the response **headers**. `None` disables it.
    ///
    /// This intentionally does not bound the response body: a streamed
    /// response may legitimately stay open for minutes.
    pub request_timeout: Option<Duration>,
    /// Idle deadline between response body frames. `None` disables it.
    pub read_timeout: Option<Duration>,
    /// How long an idle connection stays in the pool.
    pub pool_idle_timeout: Option<Duration>,
    /// Maximum idle connections kept per host.
    pub pool_max_idle_per_host: usize,
    /// Advertise HTTP/2 through ALPN; HTTP/1.1 remains available.
    pub http2: bool,
    /// Force HTTP/2 only. For cleartext targets this uses h2c prior-knowledge
    /// (no upgrade); for TLS targets it advertises only `h2` via ALPN.
    ///
    /// Disabled by default. Enable only when the upstream is known to speak
    /// HTTP/2, since this forbids HTTP/1.1 entirely.
    pub http2_only: bool,
    /// Set `TCP_NODELAY` on outbound sockets (recommended for SSE).
    pub tcp_nodelay: bool,
    /// `User-Agent` sent unless the request or default headers set one.
    pub user_agent: String,
    /// Headers applied to every request; per-request headers override them.
    pub default_headers: Vec<(String, String)>,
    /// Forward every request through this proxy URL (`http://host:port`).
    ///
    /// The client opens an HTTP `CONNECT` tunnel to the proxy, so both plain
    /// HTTP and HTTPS destinations work. `None` connects directly. This is an
    /// explicit setting; `HTTP(S)_PROXY` environment variables are not read.
    pub proxy: Option<String>,
    /// `Proxy-Authorization` value for the `CONNECT` request (for example
    /// `Basic dXNlcjpwYXNz`). Only used when [`HttpClientOptions::proxy`] is set.
    pub proxy_authorization: Option<String>,
    /// PEM file with additional root certificates (enterprise CA).
    ///
    /// When set, these roots replace the built-in webpki roots. A missing or
    /// empty PEM file fails client construction instead of silently falling
    /// back to the public roots. `None` uses the webpki bundle.
    pub root_certificates: Option<PathBuf>,
    /// Reject private/link-local/metadata destinations (SSRF guard).
    ///
    /// Enabled by default. Literal private IPs, link-local (for example the
    /// cloud metadata address `169.254.169.254`) and internal host suffixes
    /// (`.local`, `.internal`, `.home.arpa`) are rejected with
    /// [`HttpClientError::BlockedAddress`](crate::HttpClientError::BlockedAddress).
    /// Loopback (`127.0.0.0/8`, `::1`, `localhost`) stays allowed for local
    /// development and tests.
    ///
    /// The guard checks the URI host before connecting and re-checks every
    /// resolved address at connect time (closing the DNS-rebinding gap) for
    /// direct connections. When a proxy is configured the proxy resolves the
    /// CONNECT target, so operators must trust the proxy to enforce its own
    /// egress policy. Redirects are not followed by this client, so they
    /// cannot bypass the guard.
    pub ssrf_protection: bool,
    /// Allow private/link-local destinations (local vLLM/Ollama, intranet APIs).
    ///
    /// Only meaningful when [`HttpClientOptions::ssrf_protection`] is enabled.
    pub allow_private_networks: bool,
}

impl HttpClientOptions {
    /// Options with framework defaults for `base_url`.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Self::default()
        }
    }
}

impl Default for HttpClientOptions {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            connect_timeout: Duration::from_secs(3),
            request_timeout: Some(Duration::from_secs(120)),
            read_timeout: Some(Duration::from_secs(120)),
            pool_idle_timeout: Some(Duration::from_secs(90)),
            pool_max_idle_per_host: 64,
            http2: true,
            http2_only: false,
            tcp_nodelay: true,
            user_agent: format!("rust-webx-client/{}", env!("CARGO_PKG_VERSION")),
            default_headers: Vec::new(),
            proxy: None,
            proxy_authorization: None,
            root_certificates: None,
            ssrf_protection: true,
            allow_private_networks: false,
        }
    }
}

/// A request to send through an [`IHttpClient`].
///
/// `path` is relative to the client's `base_url` (`"/v1/chat/completions"`) or
/// an absolute `http(s)://` URL. Headers are stored as strings and validated
/// when the request is executed.
#[derive(Debug)]
pub struct OutboundRequest {
    method: Method,
    path: String,
    headers: Vec<(String, String)>,
    body: BodySource,
    timeout: Option<Duration>,
}

impl OutboundRequest {
    /// A request with an explicit method and path.
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            headers: Vec::new(),
            body: BodySource::Bytes(Bytes::new()),
            timeout: None,
        }
    }

    /// `GET path`.
    pub fn get(path: impl Into<String>) -> Self {
        Self::new(Method::GET, path)
    }

    /// `POST path`.
    pub fn post(path: impl Into<String>) -> Self {
        Self::new(Method::POST, path)
    }

    /// `PUT path`.
    pub fn put(path: impl Into<String>) -> Self {
        Self::new(Method::PUT, path)
    }

    /// `DELETE path`.
    pub fn delete(path: impl Into<String>) -> Self {
        Self::new(Method::DELETE, path)
    }

    /// Add a header. A later value for the same name overrides an earlier one.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Set the request body bytes.
    pub fn body(mut self, body: impl Into<Bytes>) -> Self {
        self.body = BodySource::Bytes(body.into());
        self
    }

    /// Set a streaming request body (chunks are sent as they arrive).
    pub fn body_stream<S>(mut self, stream: S) -> Self
    where
        S: Stream<Item = Result<Bytes, HttpClientError>> + Send + 'static,
    {
        self.body = BodySource::Stream(Box::pin(stream));
        self
    }

    /// Serialize `value` as the JSON body and set `content-type: application/json`.
    pub fn json<T: serde::Serialize>(mut self, value: &T) -> Result<Self, HttpClientError> {
        let body = serde_json::to_vec(value).map_err(|err| {
            HttpClientError::InvalidRequest(format!("cannot serialize JSON body: {err}"))
        })?;
        self.body = BodySource::Bytes(Bytes::from(body));
        self.headers
            .push(("content-type".into(), "application/json".into()));
        Ok(self)
    }

    // ── Read accessors (fake clients, middleware, logging) ────────────────

    /// The request method.
    pub fn method(&self) -> &Method {
        &self.method
    }

    /// The request path / absolute URL, exactly as passed to the constructor.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Headers in insertion order; a later duplicate wins when the request is
    /// sent, matching [`Self::header`].
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// The buffered request body, when the request is not streamed.
    ///
    /// `None` for a streamed body: it cannot be inspected without consuming it.
    pub fn body_bytes(&self) -> Option<&[u8]> {
        match &self.body {
            BodySource::Bytes(bytes) => Some(bytes),
            BodySource::Stream(_) => None,
        }
    }

    /// The per-request timeout override, when one was set.
    ///
    /// Named `request_timeout` because [`Self::timeout`] is the builder setter.
    pub fn request_timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// Override this client's request (header) deadline.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// The port applications depend on; tests can register a mock implementation.
///
/// A fake returns a buffered [`OutboundResponse`] built with
/// [`OutboundResponse::from_bytes`] instead of opening a socket.
#[async_trait]
pub trait IHttpClient: Send + Sync {
    /// The registry name, e.g. `"openai"`.
    fn name(&self) -> &str;

    /// Send `request` and return the response as soon as the headers arrive.
    ///
    /// A non-2xx status is returned as a normal [`OutboundResponse`]; only
    /// transport failures are errors.
    async fn execute(&self, request: OutboundRequest) -> Result<OutboundResponse, HttpClientError>;
}

/// A response whose body is still streaming.
pub struct OutboundResponse {
    /// HTTP status code.
    pub status: u16,
    /// Negotiated protocol version (`HTTP_2` when ALPN selected h2).
    pub version: http::Version,
    /// Response headers.
    pub headers: HeaderMap,
    /// Streaming response body. Read it with [`ResponseStream::collect`],
    /// [`ResponseStream::bytes`] or [`ResponseStream::sse`].
    pub body: ResponseStream,
}

impl std::fmt::Debug for OutboundResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboundResponse")
            .field("status", &self.status)
            .field("version", &self.version)
            .field("headers", &self.headers)
            .field("body", &self.body)
            .finish()
    }
}

impl std::fmt::Debug for ResponseStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseStream").finish_non_exhaustive()
    }
}

impl OutboundResponse {
    /// Whether the status is in the 2xx range.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Build a fully buffered response (tests, caches, adapters, replay).
    ///
    /// Headers are validated: an illegal name or value returns
    /// [`HttpClientError::InvalidRequest`]. The body has no inter-frame
    /// deadline, and [`ResponseStream::collect`] still enforces its limit.
    pub fn from_bytes(
        status: u16,
        headers: Vec<(String, String)>,
        body: impl Into<Bytes>,
    ) -> Result<Self, HttpClientError> {
        let mut map = HeaderMap::with_capacity(headers.len());
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                HttpClientError::InvalidRequest(format!("invalid header name: {error}"))
            })?;
            let value = HeaderValue::from_str(&value).map_err(|error| {
                HttpClientError::InvalidRequest(format!("invalid header value: {error}"))
            })?;
            map.append(name, value);
        }
        Ok(Self {
            status,
            version: http::Version::HTTP_11,
            headers: map,
            body: ResponseStream::from_bytes(body),
        })
    }
}

/// Where a [`ResponseStream`]'s bytes come from: a live upstream body or an
/// in-memory buffer supplied by [`ResponseStream::from_bytes`].
enum ResponseBody {
    Http(Incoming),
    Buffered(Bytes),
}

/// A chunked upstream body.
pub struct ResponseStream {
    inner: ResponseBody,
    read_timeout: Option<Duration>,
}

impl ResponseStream {
    /// Build from bytes already read into memory (tests, caches, adapters).
    ///
    /// The buffered stream yields the whole body as one chunk and has no
    /// inter-frame deadline (`read_timeout` is `None`); [`collect`](Self::collect)
    /// still enforces its limit.
    pub fn from_bytes(bytes: impl Into<Bytes>) -> Self {
        Self {
            inner: ResponseBody::Buffered(bytes.into()),
            read_timeout: None,
        }
    }

    /// Collect the whole body, failing above `limit` bytes.
    ///
    /// Use a small limit for error bodies and a deliberate one for JSON
    /// responses; never collect an event stream.
    pub async fn collect(mut self, limit: usize) -> Result<Bytes, HttpClientError> {
        let mut out = BytesMut::new();
        while let Some(chunk) = self.next_data().await? {
            if out.len().saturating_add(chunk.len()) > limit {
                return Err(HttpClientError::BodyTooLarge { limit });
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out.freeze())
    }

    /// Collect the whole body as UTF-8 text, failing above `limit` bytes.
    pub async fn text(self, limit: usize) -> Result<String, HttpClientError> {
        let bytes = self.collect(limit).await?;
        String::from_utf8(bytes.into())
            .map_err(|error| HttpClientError::Body(format!("response body is not UTF-8: {error}")))
    }

    /// Collect and deserialize the whole body as JSON, failing above `limit` bytes.
    pub async fn json<T: serde::de::DeserializeOwned>(
        self,
        limit: usize,
    ) -> Result<T, HttpClientError> {
        let bytes = self.collect(limit).await?;
        serde_json::from_slice(&bytes)
            .map_err(|error| HttpClientError::Body(format!("invalid JSON response body: {error}")))
    }

    /// Stream raw body chunks.
    pub fn bytes(self) -> impl Stream<Item = Result<Bytes, HttpClientError>> + Send + 'static {
        futures_util::stream::unfold(Some(self), |state| async move {
            let mut stream = state?;
            match stream.next_data().await {
                Ok(Some(chunk)) => Some((Ok(chunk), Some(stream))),
                Ok(None) => None,
                Err(error) => Some((Err(error), None)),
            }
        })
    }

    /// Decode the body as server-sent events.
    ///
    /// The decoder keeps backpressure: it only polls the upstream body when the
    /// consumer asks for the next event.
    pub fn sse(self) -> SseStream {
        SseStream::new(self.bytes())
    }

    async fn next_data(&mut self) -> Result<Option<Bytes>, HttpClientError> {
        let read_timeout = self.read_timeout;
        match &mut self.inner {
            ResponseBody::Buffered(buffer) => {
                let chunk = std::mem::take(buffer);
                Ok(if chunk.is_empty() { None } else { Some(chunk) })
            }
            ResponseBody::Http(incoming) => loop {
                let frame = match read_timeout {
                    Some(deadline) => timeout(deadline, incoming.frame())
                        .await
                        .map_err(|_| HttpClientError::Timeout(deadline))?,
                    None => incoming.frame().await,
                };

                let Some(frame) = frame else {
                    return Ok(None);
                };
                let frame = frame.map_err(|error| HttpClientError::Body(error.to_string()))?;
                if let Ok(data) = frame.into_data() {
                    return Ok(Some(data));
                }
                // Trailers and other non-data frames carry no body bytes.
            },
        }
    }
}

/// A pooled HTTP client for one provider/base URL.
pub struct HttpClient {
    name: String,
    base: Uri,
    inner: InnerClient,
    options: HttpClientOptions,
}

impl HttpClient {
    /// Build a client for `name` (the registry key) from `options`.
    ///
    /// Fails when `base_url` is not an absolute URI, the proxy URL or custom
    /// root certificate is invalid, or the TLS connector cannot be built.
    pub fn new(
        name: impl Into<String>,
        options: HttpClientOptions,
    ) -> Result<Self, HttpClientError> {
        let base: Uri = options.base_url.parse().map_err(|error| {
            HttpClientError::InvalidUrl(format!("{}: {error}", options.base_url))
        })?;
        if base.scheme().is_none() || base.authority().is_none() {
            return Err(HttpClientError::InvalidUrl(format!(
                "base URL must be absolute (scheme + authority): {}",
                options.base_url
            )));
        }
        check_outbound_uri(&base, &options)?;

        let guard_dns = options.ssrf_protection && !options.allow_private_networks;
        let mut http = HttpConnector::new_with_resolver(GuardedResolver::new(guard_dns));
        http.set_connect_timeout(Some(options.connect_timeout));
        http.set_nodelay(options.tcp_nodelay);
        // The HTTPS connector handles TLS and then hands the plain TCP stream
        // to this inner connector; it must not reject the original https URI.
        http.enforce_http(false);

        let tls = build_tls_config(&options)?;
        let builder = HttpsConnectorBuilder::new()
            .with_tls_config(tls)
            .https_or_http();
        let connector = match (options.http2_only, options.http2) {
            (true, _) => builder.enable_http2().wrap_connector(http),
            (false, true) => builder.enable_http1().enable_http2().wrap_connector(http),
            (false, false) => builder.enable_http1().wrap_connector(http),
        };

        let mut client_builder = HyperClient::builder(TokioExecutor::new());
        client_builder.pool_idle_timeout(options.pool_idle_timeout);
        client_builder.pool_max_idle_per_host(options.pool_max_idle_per_host);
        client_builder.http2_only(options.http2_only);

        let inner = match &options.proxy {
            Some(proxy) => {
                let proxy_uri: Uri = proxy.parse().map_err(|error| {
                    HttpClientError::InvalidUrl(format!("proxy URL {proxy}: {error}"))
                })?;
                if proxy_uri.scheme().is_none() || proxy_uri.authority().is_none() {
                    return Err(HttpClientError::InvalidUrl(format!(
                        "proxy URL must be absolute (scheme + authority): {proxy}"
                    )));
                }
                let mut tunnel = Tunnel::new(proxy_uri, connector);
                if let Some(authorization) = &options.proxy_authorization {
                    let value = HeaderValue::from_str(authorization).map_err(|error| {
                        HttpClientError::InvalidRequest(format!(
                            "invalid proxy-authorization value: {error}"
                        ))
                    })?;
                    tunnel = tunnel.with_auth(value);
                }
                InnerClient::Proxied(client_builder.build(tunnel))
            }
            None => InnerClient::Direct(client_builder.build(connector)),
        };

        Ok(Self {
            name: name.into(),
            base,
            inner,
            options,
        })
    }

    /// The registry name of this client.
    pub fn name(&self) -> &str {
        &self.name
    }

    fn resolve(&self, path: &str) -> Result<Uri, HttpClientError> {
        let uri = if path.starts_with("http://") || path.starts_with("https://") {
            path.parse()
                .map_err(|error| HttpClientError::InvalidUrl(format!("{path}: {error}")))?
        } else {
            let base = self.base.to_string();
            let base = base.trim_end_matches('/');
            let path = path.trim_start_matches('/');
            let joined = if path.is_empty() {
                base.to_string()
            } else if path.starts_with('?') {
                format!("{base}{path}")
            } else {
                format!("{base}/{path}")
            };
            joined
                .parse()
                .map_err(|error| HttpClientError::InvalidUrl(format!("{joined}: {error}")))?
        };
        check_outbound_uri(&uri, &self.options)?;
        Ok(uri)
    }
}

#[async_trait]
impl IHttpClient for HttpClient {
    fn name(&self) -> &str {
        &self.name
    }

    async fn execute(&self, request: OutboundRequest) -> Result<OutboundResponse, HttpClientError> {
        let OutboundRequest {
            method,
            path,
            headers,
            body,
            timeout: request_timeout,
        } = request;

        let uri = self.resolve(&path)?;

        // RFC 9110: a `Connection` header may also name additional hop-by-hop
        // fields; collect those tokens before filtering.
        let mut connection_tokens: Vec<String> = Vec::new();
        for (name, value) in self.options.default_headers.iter().chain(headers.iter()) {
            if name.eq_ignore_ascii_case("connection") {
                connection_tokens.extend(
                    value
                        .split(',')
                        .map(|token| token.trim().to_ascii_lowercase())
                        .filter(|token| !token.is_empty()),
                );
            }
        }

        let mut merged = HeaderMap::new();
        for (name, value) in self.options.default_headers.iter().chain(headers.iter()) {
            let lower = name.to_ascii_lowercase();
            if HOP_BY_HOP.contains(&lower.as_str()) || connection_tokens.contains(&lower) {
                continue;
            }
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                HttpClientError::InvalidRequest(format!("invalid header name: {error}"))
            })?;
            let value = HeaderValue::from_str(value).map_err(|error| {
                HttpClientError::InvalidRequest(format!("invalid header value: {error}"))
            })?;
            merged.insert(name, value);
        }
        if !merged.contains_key(USER_AGENT) && !self.options.user_agent.is_empty() {
            let value = HeaderValue::from_str(&self.options.user_agent).map_err(|error| {
                HttpClientError::InvalidRequest(format!("invalid user-agent: {error}"))
            })?;
            merged.insert(USER_AGENT, value);
        }
        if !merged.contains_key("traceparent") {
            if let Some(value) = outgoing_traceparent() {
                if let Ok(value) = HeaderValue::from_str(&value) {
                    merged.insert(HeaderName::from_static("traceparent"), value);
                }
            }
        }

        let mut request = Request::new(into_client_body(body));
        *request.method_mut() = method;
        *request.uri_mut() = uri;
        *request.headers_mut() = merged;

        let deadline = request_timeout.or(self.options.request_timeout);
        let response = match &self.inner {
            InnerClient::Direct(client) => send(client, request, deadline).await?,
            InnerClient::Proxied(client) => send(client, request, deadline).await?,
        };

        let (parts, body) = response.into_parts();
        Ok(OutboundResponse {
            status: parts.status.as_u16(),
            version: parts.version,
            headers: parts.headers,
            body: ResponseStream {
                inner: ResponseBody::Http(body),
                read_timeout: self.options.read_timeout,
            },
        })
    }
}

async fn send<C>(
    client: &HyperClient<C, ClientBody>,
    request: Request<ClientBody>,
    deadline: Option<Duration>,
) -> Result<hyper::Response<Incoming>, HttpClientError>
where
    C: Connect + Clone + Send + Sync + 'static,
{
    let response = match deadline {
        Some(deadline) => timeout(deadline, client.request(request))
            .await
            .map_err(|_| HttpClientError::Timeout(deadline))?,
        None => client.request(request).await,
    };
    response.map_err(|error| {
        let mut message = error.to_string();
        let mut source = std::error::Error::source(&error);
        while let Some(cause) = source {
            message.push_str(": ");
            message.push_str(&cause.to_string());
            source = std::error::Error::source(cause);
        }
        if error.is_connect() {
            HttpClientError::Connect(message)
        } else {
            HttpClientError::Protocol(message)
        }
    })
}

/// SSRF guard: reject destinations that are private, link-local, metadata or
/// internal by literal host. Loopback stays reachable for local development.
fn check_outbound_uri(uri: &Uri, options: &HttpClientOptions) -> Result<(), HttpClientError> {
    if !options.ssrf_protection || options.allow_private_networks {
        return Ok(());
    }
    let Some(host) = uri.host() else {
        return Ok(());
    };
    let host = host.trim_matches(['[', ']']);

    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if is_blocked_ip(ip) {
            return Err(HttpClientError::BlockedAddress(host.to_string()));
        }
        return Ok(());
    }

    let lower = host.to_ascii_lowercase();
    if lower.ends_with(".local")
        || lower.ends_with(".internal")
        || lower.ends_with(".home.arpa")
        || lower == "metadata.google.internal"
    {
        return Err(HttpClientError::BlockedAddress(host.to_string()));
    }
    Ok(())
}

fn is_blocked_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => is_blocked_ipv4(ip),
        std::net::IpAddr::V6(ip) => {
            // IPv4-mapped (::ffff:a.b.c.d) and IPv4-compatible (::a.b.c.d)
            // addresses tunnel IPv4 destinations through an IpAddr::V6. Map
            // them back and apply the full IPv4 policy — otherwise a one-token
            // `::ffff:` prefix reaches the cloud metadata service.
            if let Some(mapped) = ipv4_mapped(ip) {
                return is_blocked_ipv4(mapped);
            }
            let segments = ip.segments();
            let first = segments[0];
            let second = segments[1];
            !ip.is_loopback()
                && (ip.is_unspecified()
                    // Unique local fc00::/7
                    || (first & 0xfe00) == 0xfc00
                    // Link-local fe80::/10
                    || (first & 0xffc0) == 0xfe80
                    // Multicast ff00::/8
                    || (first & 0xff00) == 0xff00
                    // Documentation 2001:db8::/32
                    || (first == 0x2001 && second == 0x0db8)
                    // Deprecated site-local fec0::/10
                    || (first & 0xffc0) == 0xfec0
                    // 6to4 2002::/16 (can embed private IPv4)
                    || first == 0x2002
                    // NAT64 well-known prefix 64:ff9b::/96 (check embedded v4)
                    || is_nat64_blocked(segments))
        }
    }
}

/// Return the embedded IPv4 for `::ffff:a.b.c.d` (mapped) and `::a.b.c.d`
/// (compatible), but not for `::` / `::1`.
fn ipv4_mapped(ip: std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let s = ip.segments();
    if s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0 {
        if s[5] == 0xffff {
            // ::ffff:a.b.c.d — but exclude ::ffff:0:0/96 which is a /96 network,
            // still map it so the v4 policy (0.0.0.0/8) blocks it.
            return Some(std::net::Ipv4Addr::new(
                (s[6] >> 8) as u8,
                s[6] as u8,
                (s[7] >> 8) as u8,
                s[7] as u8,
            ));
        }
        if s[5] == 0 && (s[6] != 0 || s[7] > 1) {
            // ::a.b.c.d compatible form (skip :: and ::1 handled by caller).
            return Some(std::net::Ipv4Addr::new(
                (s[6] >> 8) as u8,
                s[6] as u8,
                (s[7] >> 8) as u8,
                s[7] as u8,
            ));
        }
    }
    None
}

/// NAT64 `64:ff9b::/96` embeds an IPv4 in the **last two** segments (`s[6]`,
/// `s[7]`), because /96 leaves the final 32 bits for the IPv4 address. Apply the
/// v4 policy to those embedded bits so `64:ff9b::a9fe:a9fe` (169.254.169.254)
/// cannot reach metadata, while a public `64:ff9b::0808:0808` (8.8.8.8) is
/// allowed. Using `s[5]`/`s[6]` would shift the address one segment left,
/// falsely blocking every mapped public address as `0.0.x.y`.
fn is_nat64_blocked(s: [u16; 8]) -> bool {
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0 {
        let v4 =
            std::net::Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
        return is_blocked_ipv4(v4);
    }
    false
}

fn is_blocked_ipv4(ip: std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    !ip.is_loopback()
        && (ip.is_private()
            || ip.is_link_local()
            || ip.is_unspecified()
            || ip.is_broadcast()
            || ip.is_documentation()
            // 0.0.0.0/8, 100.64/10 (CGNAT), 198.18/15 (benchmark),
            // 192.0.0/24 (IETF protocol assignments).
            || octets[0] == 0
            || (octets[0] == 100 && (octets[1] & 0xc0) == 64)
            || (octets[0] == 198 && (octets[1] & 0xfe) == 18)
            || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0))
}

/// Build the rustls client config: webpki roots, or a PEM bundle that replaces
/// them (enterprise CA).
fn build_tls_config(options: &HttpClientOptions) -> Result<rustls::ClientConfig, HttpClientError> {
    // A workspace can enable both `ring` and `aws-lc-rs`; rustls then refuses
    // to pick a process provider automatically. The framework pins `ring`;
    // an application that installed its own provider first keeps it.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut roots = RootCertStore::empty();
    match &options.root_certificates {
        Some(path) => {
            let file = std::fs::File::open(path).map_err(|error| {
                HttpClientError::InvalidRequest(format!(
                    "cannot read root certificates {}: {error}",
                    path.display()
                ))
            })?;
            let mut reader = std::io::BufReader::new(file);
            let mut added = 0usize;
            for certificate in CertificateDer::pem_reader_iter(&mut reader) {
                let certificate = certificate.map_err(|error| {
                    HttpClientError::InvalidRequest(format!(
                        "invalid PEM certificate in {}: {error}",
                        path.display()
                    ))
                })?;
                roots.add(certificate).map_err(|error| {
                    HttpClientError::InvalidRequest(format!(
                        "cannot add root certificate from {}: {error}",
                        path.display()
                    ))
                })?;
                added += 1;
            }
            if added == 0 {
                return Err(HttpClientError::InvalidRequest(format!(
                    "root certificate file {} contains no certificates",
                    path.display()
                )));
            }
        }
        None => roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned()),
    }

    Ok(rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth())
}
