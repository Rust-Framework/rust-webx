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
use hyper_util::client::legacy::connect::proxy::Tunnel;
use hyper_util::client::legacy::connect::{Connect, HttpConnector};
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::rt::TokioExecutor;
use rustls::RootCertStore;
use std::fmt;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::timeout;

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
type Https = HttpsConnector<HttpConnector>;
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
            tcp_nodelay: true,
            user_agent: format!("rust-webx-client/{}", env!("CARGO_PKG_VERSION")),
            default_headers: Vec::new(),
            proxy: None,
            proxy_authorization: None,
            root_certificates: None,
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

    /// Override this client's request (header) deadline.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// The port applications depend on; tests can register a mock implementation.
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
}

/// A chunked upstream body.
pub struct ResponseStream {
    inner: Incoming,
    read_timeout: Option<Duration>,
}

impl ResponseStream {
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
        loop {
            let frame = match self.read_timeout {
                Some(deadline) => timeout(deadline, self.inner.frame())
                    .await
                    .map_err(|_| HttpClientError::Timeout(deadline))?,
                None => self.inner.frame().await,
            };

            let Some(frame) = frame else {
                return Ok(None);
            };
            let frame = frame.map_err(|error| HttpClientError::Body(error.to_string()))?;
            if let Ok(data) = frame.into_data() {
                return Ok(Some(data));
            }
            // Trailers and other non-data frames carry no body bytes.
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

        let mut http = HttpConnector::new();
        http.set_connect_timeout(Some(options.connect_timeout));
        http.set_nodelay(options.tcp_nodelay);

        let tls = build_tls_config(&options)?;
        let builder = HttpsConnectorBuilder::new()
            .with_tls_config(tls)
            .https_or_http();
        let connector = if options.http2 {
            builder.enable_http1().enable_http2().wrap_connector(http)
        } else {
            builder.enable_http1().wrap_connector(http)
        };

        let mut client_builder = HyperClient::builder(TokioExecutor::new());
        client_builder.pool_idle_timeout(options.pool_idle_timeout);
        client_builder.pool_max_idle_per_host(options.pool_max_idle_per_host);

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
        if path.starts_with("http://") || path.starts_with("https://") {
            return path
                .parse()
                .map_err(|error| HttpClientError::InvalidUrl(format!("{path}: {error}")));
        }

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
            .map_err(|error| HttpClientError::InvalidUrl(format!("{joined}: {error}")))
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
            headers: parts.headers,
            body: ResponseStream {
                inner: body,
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
        if error.is_connect() {
            HttpClientError::Connect(error.to_string())
        } else {
            HttpClientError::Protocol(error.to_string())
        }
    })
}

/// Build the rustls client config: webpki roots, or a PEM bundle that replaces
/// them (enterprise CA).
fn build_tls_config(options: &HttpClientOptions) -> Result<rustls::ClientConfig, HttpClientError> {
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
            for certificate in rustls_pemfile::certs(&mut reader) {
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
