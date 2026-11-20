# Engine 2.0 — frozen internal interfaces (M1)

This file is the contract between the new engine modules. Implementers may add
private helpers, but must keep these public signatures stable so the host
adapter, macros and tests can be written in parallel.

Crate: `crates/engine` (package `rust-webx-engine`, lib `webx_engine`).
All errors use `webx_core::error::{Error, Result}`.

## config.rs

```rust
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub workers: usize,                 // 0 = auto
    pub max_connections: usize,
    pub header_read_timeout: Duration,
    pub max_header_buf: usize,
    pub drain_timeout: Duration,
    pub stream_drain_timeout: Duration,
    pub request_timeout: Option<Duration>,
    pub tls: Option<TlsConfig>,
    pub mode: AppMode,
}

#[derive(Debug, Clone)]
pub struct TlsConfig { pub cert_path: String, pub key_path: String }

impl EngineConfig {
    pub fn from_options(options: &AppOptions) -> Self;
    pub fn resolved_workers(&self) -> usize;
}
```

## body.rs

`BodyLimits` moves here (host re-exports it):

```rust
#[derive(Debug, Clone)]
pub struct BodyLimits { body, multipart_body, file, field, memory_threshold, spool_dir }
impl BodyLimits { pub fn from_options(&AppOptions) -> Self; pub fn apply_spool_root(&self); }
```

## router.rs

```rust
// Alias of webx_core::route::scan::HandlerDispatchFn in the implementation:
pub type DirectDispatchFn = for<'a> fn(
    &'a mut dyn IHttpContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<EndpointResult>> + Send + 'a>>;

#[async_trait::async_trait]
pub trait RouteTarget: Send + Sync {
    async fn handle(&self, ctx: &mut EngineContext) -> Result<()>;
}

pub struct DirectTarget { pub dispatch: DirectDispatchFn }
pub struct EndpointTarget { pub endpoint: Arc<dyn IEndpoint> }

pub struct RouteMatch {
    pub target: Arc<dyn RouteTarget>,
    pub pattern: Arc<str>,
    pub params: Vec<(&'static str, String)>, // empty for static routes
}

pub struct Router { /* matchit per method + fallback */ }
impl Router {
    pub fn new() -> Self;
    pub fn insert(&mut self, method: HttpMethod, path: &'static str, target: Arc<dyn RouteTarget>);
    pub fn set_fallback(&mut self, target: Arc<dyn RouteTarget>);
    pub fn match_route(&self, method: &str, path: &str) -> Option<RouteMatch>;
    pub fn path_exists(&self, path: &str) -> bool;
    pub fn allowed_methods(&self, path: &str) -> Vec<&'static str>;
}
```

`RouteTarget::handle` writes the response into `ctx` (via
`ctx.write_endpoint_result` or `IHttpResponse` methods).

## context.rs / response.rs

```rust
pub enum EndpointResult {
    Empty,
    Json { status: u16, body: bytes::Bytes },
    Action(webx_core::action::Response),
}

pub struct EngineContext { /* request parts, body, route, claims, trace, response */ }
impl EngineContext {
    pub fn new(
        req: hyper::Request<hyper::body::Incoming>,
        limits: Arc<BodyLimits>,
        peer: Option<std::net::SocketAddr>,
        dispatch: Arc<webx_core::dispatch_runtime::DispatchRuntime>,
    ) -> Self;
    pub fn set_route(&mut self, pattern: Arc<str>, params: Vec<(&'static str, String)>);
    pub async fn write_endpoint_result(&mut self, result: EndpointResult) -> Result<()>;
    pub async fn finalize(self) -> hyper::Response<RespBody>;
}

pub type RespBody = http_body_util::combinators::UnsyncBoxBody<bytes::Bytes, std::io::Error>;
```

`EngineContext` implements `IHttpContext`, `IClaimsExt`, and its inner request /
response implement `IHttpRequest` / `IHttpResponse`. `IHttpRequest::body_bytes_shared`
may return `Bytes`; `IHttpResponse::append_header` must be supported.

## pipeline.rs

```rust
pub struct Pipeline { pub stages: Vec<Arc<dyn IMiddleware>> }
impl Pipeline {
    pub fn new(stages: Vec<Arc<dyn IMiddleware>>) -> Self;
    pub async fn execute(&self, ctx: &mut EngineContext) -> Result<()>;
}
```

`execute` runs stages in order, calls the router terminal (stored in
`EngineContext`? no — the pipeline receives a `Handler` terminal). Exact shape:

```rust
pub type FinalHandler = Arc<
    dyn for<'a> Fn(&'a mut EngineContext)
        -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>
        + Send + Sync,
>;

impl Pipeline {
    pub async fn execute(&self, ctx: &mut EngineContext, final_handler: FinalHandler) -> Result<()>;
}
```

Semantics identical to 1.5: forward `invoke`, short-circuit, final handler,
reverse `after`, first error wins, after-hook errors are logged.

## shared.rs

```rust
pub struct Shared {
    pub config: Arc<EngineConfig>,
    pub router: Arc<Router>,
    pub pipeline: Arc<Pipeline>,
    pub dispatch: Arc<DispatchRuntime>,
    pub body_limits: Arc<BodyLimits>,
    pub error_writers: Arc<Vec<Arc<dyn IErrorResponseWriter>>>,
    pub tls: Option<Arc<rustls::ServerConfig>>,
    pub shutdown: tokio::sync::watch::Sender<bool>,
    pub shutdown_token: webx_core::hosted::CancellationToken,
    pub active_streams: Arc<std::sync::atomic::AtomicUsize>,
}
```

## server.rs / worker.rs

```rust
pub struct Server { pub shared: Arc<Shared> }
impl Server {
    pub fn new(shared: Arc<Shared>) -> Self;
    pub fn handle(&self) -> ServerHandle;
    pub async fn run(self, listener: std::net::TcpListener) -> Result<()>;
}

#[derive(Clone)]
pub struct ServerHandle {
    pub shutdown: tokio::sync::watch::Sender<bool>,
    pub token: CancellationToken,
}
impl ServerHandle { pub fn shutdown(&self); }
```

`run` splits into one acceptor on the caller runtime and N worker threads
(`current_thread` runtimes). Conn handoff is safe:
`tokio::net::TcpStream::into_std()` on the acceptor, then
`tokio::net::TcpStream::from_std()` on the worker. No `unsafe`.

## tls.rs

```rust
pub fn build_server_config(cert_path: &str, key_path: &str) -> Result<Arc<rustls::ServerConfig>>;
```

## Host adapter contract

`webx_host::HostBuilder` builds `Shared` and calls `Server::new(shared).run(listener)`.
The host owns auth/CORS/health/SPA/OpenAPI middleware and endpoints; the engine
only knows `IMiddleware`, `RouteTarget`, `EngineContext`, `EndpointResult` and
`Shared`.
