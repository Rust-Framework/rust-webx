//! Host builder and hyper server integration.
//!
//! Includes built-in exception middleware: errors produced by endpoints
//! are caught and converted to well-formed HTTP error responses using
//! `Error::status_code()`.

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::TokioIo;
use rust_dicore::{ServiceCollection, ServiceProvider};
use rust_webapp_core::app::IHost;
use rust_webapp_core::config::{self, AppOptions};
use rust_webapp_core::error::Result;
use rust_webapp_core::handler::IHostedService;
use rust_webapp_core::http::IHttpContext;
use rust_webapp_core::middleware::IMiddleware;
use rust_webapp_core::mode::AppMode;
use rust_webapp_core::routing::{HttpMethod, IEndpoint, IRouter};

use crate::auth_jwt::{init_jwt_secret, jwt_middleware, JwtAuth};
use crate::authz::collect_authorizers;
use crate::context::HttpContext;
use crate::cors::{CorsConfig, CorsMiddleware};
use crate::endpoint::{StaticHtmlEndpoint, StaticJsonEndpoint, StubEndpoint};
use crate::memory_cache::MemoryCache;
use crate::pipeline::{HandlerFn, MiddlewarePipeline};
use crate::router::Router;
use jsonwebtoken::{DecodingKey, Validation};
use rust_webapp_core::di::scan::RouteEntry;
use rust_webapp_openapi::{generate_openapi_spec, APIUI_HTML};
use rust_webapp_spa::SpaMiddleware;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;

pub struct Host {
    #[allow(dead_code)]
    provider: Arc<ServiceProvider>,
    pub options: AppOptions,
    pipeline: Arc<MiddlewarePipeline>,
    /// The matchit router (retained for introspection).
    #[allow(dead_code)]
    router: Arc<Router>,
    /// Pre-built router handler â€?eliminates per-request Arc::new.
    router_handler: HandlerFn,
    mode: AppMode,
    #[allow(dead_code)]
    spa_root: Option<String>,
    shutdown: Arc<tokio::sync::Notify>,
    /// Hosted services that are started on host start
    /// and stopped on graceful shutdown.
    hosted_services: Vec<Arc<dyn IHostedService>>,
}

#[allow(clippy::type_complexity)]
pub struct HostBuilder {
    service_configs: Vec<Box<dyn FnOnce(ServiceCollection) -> ServiceCollection + Send>>,
    mode: AppMode,
    spa_root: Option<String>,
    options_modifiers: Vec<Box<dyn FnOnce(&mut AppOptions) + Send>>,
    cors_config: Option<CorsConfig>,
    use_auth: bool,
}

#[allow(clippy::type_complexity)]
pub struct HostAppBuilder {
    options_modifiers: Vec<Box<dyn FnOnce(&mut AppOptions) + Send>>,
}

impl HostAppBuilder {
    fn new() -> Self {
        Self {
            options_modifiers: Vec::new(),
        }
    }

    #[allow(non_snake_case)]
    pub fn useOptions<F>(&mut self, f: F)
    where
        F: FnOnce(&mut AppOptions) + Send + 'static,
    {
        self.options_modifiers.push(Box::new(f));
    }
}

impl HostBuilder {
    pub fn new() -> Self {
        Self {
            service_configs: Vec::new(),
            mode: AppMode::default(),
            spa_root: None,
            options_modifiers: Vec::new(),
            cors_config: None,
            use_auth: false,
        }
    }

    pub fn register<F>(mut self, f: F) -> Self
    where
        F: FnOnce(ServiceCollection) -> ServiceCollection + Send + 'static,
    {
        self.service_configs.push(Box::new(f));
        self
    }

    pub fn configure<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut HostAppBuilder) + Send + 'static,
    {
        let mut builder = HostAppBuilder::new();
        f(&mut builder);
        self.options_modifiers
            .append(&mut builder.options_modifiers);
        self
    }

    pub fn mode(mut self, mode: AppMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn use_spa(mut self, root: impl Into<String>) -> Self {
        self.spa_root = Some(root.into());
        self
    }

    pub fn use_cors(mut self, config: CorsConfig) -> Self {
        self.cors_config = Some(config);
        self
    }

    pub fn use_auth(mut self) -> Self {
        self.use_auth = true;
        self
    }

    /// Register a memory cache instance for use via `IDistributedCache` trait.
    ///
    /// The cache is registered as a singleton in the DI container.
    /// Handlers can access it by implementing `From<Arc<MemoryCache>>`.
    ///
    /// ```ignore
    /// Host::builder()
    ///     .use_memory_cache()
    ///     .build()
    ///     .run().await;
    /// ```
    pub fn use_memory_cache(mut self) -> Self {
        let cache = Arc::new(MemoryCache::new());
        self.service_configs
            .push(Box::new(move |svc| svc.instance(cache)));
        self
    }

    /// Register a memory cache with custom configuration.
    pub fn use_memory_cache_with(mut self, max_entries: usize) -> Self {
        let cache = Arc::new(MemoryCache::new().with_max_entries(max_entries));
        self.service_configs
            .push(Box::new(move |svc| svc.instance(Arc::clone(&cache))));
        self
    }

    pub fn build(self) -> Host {
        // Initialize structured logging based on app mode.
        // This is idempotent â€?subsequent calls are no-ops.
        let env_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into());
        if self.mode == AppMode::Development {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .without_time()
                .with_target(false)
                .try_init();
        } else {
            let _ = tracing_subscriber::fmt()
                .json()
                .with_env_filter(env_filter)
                .try_init();
        }

        let mut svc = ServiceCollection::from_injected();
        for cfg in self.service_configs {
            svc = cfg(svc);
        }

        let provider = Arc::new(svc.build().unwrap_or_else(|e| {
            panic!(
                "Failed to build ServiceProvider: {}. Check your DI registrations.",
                e
            );
        }));

        // Set the global provider so #[handler] factories can resolve DI dependencies.
        rust_webapp_core::di::scan::set_global_provider(Arc::clone(&provider));

        // Initialize the global handler cache from inventory registrations.
        // Handlers registered via #[handler] are collected into HandlerCache.
        // If a handler struct also has #[inject_attr], its factory will resolve
        // dependencies via the global provider set above.
        rust_webapp_core::di::scan::HandlerCache::init_global();

        let mut pipeline = MiddlewarePipeline::new();
        let middlewares: Vec<Arc<dyn IMiddleware>> = provider.get_all::<dyn IMiddleware>();
        for mw in middlewares {
            pipeline.add_middleware(mw);
        }

        let appsettings =
            config::load_appsettings(self.mode).unwrap_or_else(|| serde_json::json!({}));
        let mut options: AppOptions = config::bind_root(&appsettings);
        for modifier in self.options_modifiers {
            modifier(&mut options);
        }

        let cors = self.cors_config.unwrap_or_else(|| {
            let cs = &options.cors;
            CorsConfig {
                origins: cs.origins.clone(),
                methods: cs.methods.clone(),
                headers: cs.headers.clone(),
                allow_credentials: cs.allow_credentials,
                max_age: cs.max_age,
            }
        });
        pipeline.add_middleware(Arc::new(CorsMiddleware::new(cors)));

        if let Some(ref spa_root) = self.spa_root {
            pipeline.add_middleware(Arc::new(SpaMiddleware::new(spa_root.clone())));
        }

        if self.use_auth {
            let secret = options.jwt.secret.clone();
            if !secret.is_empty() {
                let jwt_auth = Arc::new(JwtAuth::new(
                    DecodingKey::from_secret(secret.as_bytes()),
                    Validation::default(),
                ));
                pipeline.add_middleware(Arc::new(jwt_middleware(jwt_auth)));
                init_jwt_secret(&secret);
            } else {
                tracing::warn!(
                    "use_auth() enabled but no JWT secret configured. Set jwt.secret in appsettings.json or JWT_SECRET env var."
                );
            }
        }

        // Users can register additional middleware here, e.g.:
        // pipeline.add_middleware(Arc::new(RequestIdMiddleware::new()));
        // pipeline.add_middleware(Arc::new(TimingMiddleware::new()));

        let pipeline = Arc::new(pipeline);

        // Security: Warn if JWT secret is the default value in production
        if self.mode == AppMode::Production
            && (options.jwt.secret.is_empty()
                || options.jwt.secret.contains("change-in-production")
                || options.jwt.secret.contains("demo-secret"))
        {
            tracing::warn!(
                "INSECURE: JWT secret is default or empty. Set a strong secret via appsettings.json or JWT_SECRET env var."
            );
        }

        // Security: Warn if CORS allows all origins in production
        if self.mode == AppMode::Production && options.cors.origins.iter().any(|o| o == "*") {
            tracing::warn!(
                "INSECURE: CORS allows all origins (*) in production. Restrict to specific origins."
            );
        }

        let mut router = Router::new();
        let mut route_count = 0usize;

        // Build dispatch map: handler_type â†?dispatch function
        #[allow(clippy::type_complexity)]
        let mut dispatch_map: std::collections::HashMap<
            &'static str,
            fn(
                Vec<u8>,
                std::collections::HashMap<String, String>,
                std::collections::HashMap<String, String>,
                Option<Box<dyn rust_webapp_core::auth::IClaims>>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = rust_webapp_core::error::Result<rust_webapp_core::di::scan::ResponseData>,
                        > + Send,
                >,
            >,
        > = std::collections::HashMap::new();

        for dispatch in inventory::iter::<rust_webapp_core::di::scan::RouteDispatch> {
            dispatch_map.insert(dispatch.handler_type, dispatch.dispatch);
        }

        for entry in inventory::iter::<RouteEntry> {
            route_count += 1;
            let stub = Arc::new(StubEndpoint {
                method: entry.method.as_str(),
                path: entry.path,
                handler_type: entry.handler_type,
                dispatch_fn: dispatch_map.get(entry.handler_type).copied(),
                auth_required_role: entry.required_role,
                authorizers: collect_authorizers(provider.as_ref()),
            });
            router.register(entry.method, entry.path, stub);
        }

        let openapi_spec = generate_openapi_spec("LRWF API", "1.0.0");
        let openapi_bytes = serde_json::to_vec(&openapi_spec).unwrap_or_default();
        router.register(
            HttpMethod::Get,
            "/api/openapi.json",
            Arc::new(StaticJsonEndpoint {
                body: openapi_bytes,
            }),
        );
        router.register(
            HttpMethod::Get,
            "/api/openapi.html",
            Arc::new(StaticHtmlEndpoint { body: APIUI_HTML }),
        );

        // Health check endpoints for monitoring / container orchestration
        let health_json =
            serde_json::to_vec(&serde_json::json!({"status":"ok"})).unwrap_or_default();
        let health_endpoint: Arc<dyn IEndpoint> =
            Arc::new(StaticJsonEndpoint { body: health_json });
        router.register(HttpMethod::Get, "/health", Arc::clone(&health_endpoint));
        router.register(HttpMethod::Get, "/healthz", health_endpoint);

        // Kubernetes liveness endpoint (always returns OK as long as process is alive)
        let live_json =
            serde_json::to_vec(&serde_json::json!({"status":"alive"})).unwrap_or_default();
        router.register(
            HttpMethod::Get,
            "/health/live",
            Arc::new(StaticJsonEndpoint { body: live_json }),
        );

        // Kubernetes readiness endpoint (checks registered health probes)
        let ready_json =
            serde_json::to_vec(&serde_json::json!({"status":"ready"})).unwrap_or_default();
        router.register(
            HttpMethod::Get,
            "/health/ready",
            Arc::new(StaticJsonEndpoint { body: ready_json }),
        );

        if self.mode == AppMode::Development {
            let version = env!("CARGO_PKG_VERSION");
            tracing::info!("");
            tracing::info!("  â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€");
            tracing::info!("    Rust WebApplication Framework v{}", version);
            tracing::info!("  â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€");
            tracing::info!("    App:      {}", options.app.name);
            tracing::info!("    CORS:     enabled");
            if let Some(ref root) = self.spa_root {
                tracing::info!("    SPA Root: {}", root);
            }
            if route_count > 0 {
                tracing::info!("    Routes:   {} registered", route_count);
            }
            let banner_urls = if options.app.urls.is_empty() {
                vec!["http://localhost:5000".to_string()]
            } else {
                options
                    .app
                    .urls
                    .iter()
                    .map(|u| u.replace("0.0.0.0", "localhost"))
                    .collect::<Vec<_>>()
            };
            for url in &banner_urls {
                tracing::info!("    OpenAPI:  {}/api/openapi.html", url);
                tracing::info!("    OpenAPI:  {}/api/openapi.json", url);
            }
            tracing::info!("  â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€");
            tracing::info!("");
        } else if route_count > 0 {
            tracing::info!("{} route(s) registered", route_count);
        }

        let router = Arc::new(router);
        let router_handler = make_router_handler(Arc::clone(&router));

        // Resolve all registered hosted services from the DI container.
        // These will be started when `run()` is called and stopped on shutdown.
        let hosted_services: Vec<Arc<dyn IHostedService>> =
            provider.get_all::<dyn IHostedService>();

        Host {
            provider,
            options,
            pipeline,
            router,
            router_handler,
            mode: self.mode,
            spa_root: self.spa_root,
            shutdown: Arc::new(tokio::sync::Notify::new()),
            hosted_services,
        }
    }
}

/// Handle for a running server, allowing programmatic graceful shutdown.
pub struct ServerHandle {
    shutdown: Arc<tokio::sync::Notify>,
}

impl ServerHandle {
    /// Signal the server to stop accepting new connections and begin
    /// draining existing ones.
    pub fn shutdown(self) {
        self.shutdown.notify_waiters();
    }
}

/// A fully built server that owns its runtime lifecycle.
///
/// Created via `Host::into_server()`. Provides a graceful shutdown
/// handle alongside the `run()` method.
///
/// ```ignore
/// let server = Host::builder().build().into_server();
/// let handle = server.handle();
/// tokio::spawn(async move { server.run().await });
/// handle.shutdown();
/// ```
pub struct Server {
    host: Host,
}

impl Server {
    /// Start listening on all configured URLs.
    pub async fn run(self) -> Result<()> {
        self.host.run().await
    }

    /// Start listening on a single address (convenience).
    pub async fn run_at(self, addr: &str) -> Result<()> {
        self.host.run_at(addr).await
    }

    /// Obtain a shutdown handle before starting.
    pub fn handle(&self) -> ServerHandle {
        self.host.server_handle()
    }
}

impl Default for HostBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Host {
    pub fn builder() -> HostBuilder {
        HostBuilder::new()
    }

    pub fn options(&self) -> &AppOptions {
        &self.options
    }

    /// Start the server on all URLs configured in AppOptions.app.Urls.
    ///
    /// Automatically detects http:// and https:// URLs from the array.
    /// Starts a plain TCP listener for each http:// URL and a TLS listener
    /// for each https:// URL. All listeners run concurrently.
    ///
    /// Supports graceful shutdown on Ctrl+C/SIGTERM with 30s connection drain.
    ///
    /// # Example
    ///
    /// ```json
    /// { "App": { "Urls": ["http://localhost:5000", "https://localhost:5030"] } }
    /// ```
    pub async fn run(&self) -> Result<()> {
        // â”€â”€ Start all hosted services before accepting connections â”€â”€
        if !self.hosted_services.is_empty() {
            tracing::info!(
                "Starting {} hosted service(s)...",
                self.hosted_services.len()
            );
            for svc in &self.hosted_services {
                svc.start().await?;
            }
            tracing::info!("All hosted services started.");
        }

        let urls = if self.options.app.urls.is_empty() {
            vec!["http://0.0.0.0:5000".to_string()]
        } else {
            self.options.app.urls.clone()
        };

        let mut http_addrs: Vec<String> = Vec::new();
        let mut https_addrs: Vec<String> = Vec::new();

        for url in &urls {
            let (scheme, addr) = parse_url(url)?;
            match scheme {
                "http" => http_addrs.push(addr),
                "https" => https_addrs.push(addr),
                other => {
                    return Err(rust_webapp_core::error::Error::Http(format!(
                        "Unsupported URL scheme '{}' in '{}'",
                        other, url
                    )))
                }
            }
        }

        let acceptor = if !https_addrs.is_empty() {
            let tls = &self.options.tls;
            if tls.cert_path.is_empty() || tls.key_path.is_empty() {
                return Err(rust_webapp_core::error::Error::Http(
                    "HTTPS URLs require Tls.CertPath and Tls.KeyPath".into(),
                ));
            }
            Some(build_tls_acceptor(&tls.cert_path, &tls.key_path)?)
        } else {
            None
        };

        // Banner
        if self.mode == AppMode::Development {
            tracing::info!("");
            for url in &urls {
                let display_url = url.replace("0.0.0.0", "localhost");
                tracing::info!("  Listening on {}", display_url);
            }
        } else {
            tracing::info!("Listening on {} url(s)", urls.len());
        }

        let notify = Arc::clone(&self.shutdown);

        let shutdown_notify = std::sync::Arc::clone(&notify);
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                let mut sigterm = signal(SignalKind::terminate())
                    .expect("Failed to register SIGTERM handler");
                let mut sigint = signal(SignalKind::interrupt())
                    .expect("Failed to register SIGINT handler");
                tokio::select! {
                    _ = sigterm.recv() => {},
                    _ = sigint.recv() => {},
                    _ = tokio::signal::ctrl_c() => {},
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            tracing::info!("Shutdown signal received, draining connections...");
            shutdown_notify.notify_waiters();
        });

        let mut handles = Vec::new();
        let pipeline = Arc::clone(&self.pipeline);
        let router_handler = self.router_handler.clone();
        let mode = self.mode;
        let max_body_size = self.options.app.max_body_size;

        for addr in &http_addrs {
            let addr = addr.clone();
            let n = std::sync::Arc::clone(&notify);
            let p = Arc::clone(&pipeline);
            let rh = router_handler.clone();
            handles.push(tokio::spawn(serve_http(
                addr,
                n,
                p,
                rh,
                mode,
                max_body_size,
            )));
        }

        if let Some(ref tls_acceptor) = acceptor {
            for addr in &https_addrs {
                let addr = addr.clone();
                let n = std::sync::Arc::clone(&notify);
                let p = Arc::clone(&pipeline);
                let rh = router_handler.clone();
                let a = tls_acceptor.clone();
                handles.push(tokio::spawn(serve_https(
                    addr,
                    a,
                    n,
                    p,
                    rh,
                    mode,
                    max_body_size,
                )));
            }
        }

        // Wait for all HTTP listeners to finish (they exit after shutdown signal).
        for h in handles {
            let _ = h.await;
        }

        // â”€â”€ Stop all hosted services during graceful shutdown â”€â”€
        if !self.hosted_services.is_empty() {
            tracing::info!(
                "Stopping {} hosted service(s)...",
                self.hosted_services.len()
            );
            for svc in &self.hosted_services {
                if let Err(e) = svc.stop().await {
                    tracing::warn!("Hosted service stop error: {}", e);
                }
            }
            tracing::info!("All hosted services stopped.");
        }

        Ok(())
    }

    /// Start the server at a single explicit address (convenience wrapper).
    pub async fn run_at(&self, addr: &str) -> Result<()> {
        let notify = Arc::clone(&self.shutdown);
        serve_http(
            addr.to_string(),
            notify,
            Arc::clone(&self.pipeline),
            self.router_handler.clone(),
            self.mode,
            self.options.app.max_body_size,
        )
        .await;
        Ok(())
    }

    /// Return a `ServerHandle` that can be used to signal graceful shutdown
    /// from application code (e.g., integration tests, health checks).
    ///
    /// ```ignore
    /// let host = Host::builder().build();
    /// let handle = host.server_handle();
    /// tokio::spawn(async move { host.run().await });
    /// // ... later:
    /// handle.shutdown();
    /// ```
    pub fn server_handle(&self) -> ServerHandle {
        ServerHandle {
            shutdown: Arc::clone(&self.shutdown),
        }
    }

    /// Consume the host and return a `Server` that owns the runtime lifecycle.
    ///
    /// The returned `Server` can be `.run().await`-ed and provides a
    /// handle for graceful shutdown.
    pub fn into_server(self) -> Server {
        Server { host: self }
    }
}

#[async_trait::async_trait]
impl IHost for Host {
    async fn run(&self, addr: &str) -> Result<()> {
        self.run_at(addr).await
    }
    async fn stop(&self) -> Result<()> {
        tracing::info!("Stop requested.");
        Ok(())
    }
}

fn make_router_handler(router: Arc<Router>) -> HandlerFn {
    Arc::new(move |ctx: &mut dyn IHttpContext| {
        let router = Arc::clone(&router);
        Box::pin(async move {
            match router.match_route(ctx).await? {
                Some((endpoint, params, pattern)) => {
                    drop(router);
                    for (key, value) in params {
                        ctx.request_mut().route_params_mut().insert(key, value);
                    }
                    *ctx.request_mut().route_pattern_mut() = Some(pattern);
                    endpoint.handle(ctx).await
                }
                None => {
                    drop(router);
                    // Don't overwrite if a middleware (e.g. SPA) already
                    // wrote a response body (static file, index.html fallback).
                    if !ctx.response().has_body() {
                        write_error_response(ctx, 404, "Not Found").await;
                    }
                    Ok(())
                }
            }
        })
    })
}

async fn handle_request(
    req: Request<Incoming>,
    pipeline: Arc<MiddlewarePipeline>,
    router_handler: HandlerFn,
    max_body_size: usize,
) -> std::result::Result<hyper::Response<Full<Bytes>>, std::convert::Infallible> {
    let mut ctx = HttpContext::new(req, max_body_size).await;
    let result = pipeline.execute(&mut ctx, router_handler).await;

    if let Err(e) = result {
        let status = e.status_code();
        write_error_response(&mut ctx, status, &e.to_string()).await;
    }

    Ok(ctx.into_response())
}

async fn write_error_response(ctx: &mut dyn IHttpContext, status: u16, message: &str) {
    ctx.response_mut().set_status(status);
    ctx.response_mut()
        .set_header("content-type", "application/problem+json");
    // RFC 7807 Problem Details format
    let title = match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    };
    let body = serde_json::json!({
        "type": format!("https://httpstatuses.com/{}", status),
        "title": title,
        "status": status,
        "detail": message,
    });
    let _ = ctx
        .response_mut()
        .write_bytes(serde_json::to_vec(&body).unwrap_or_default())
        .await;
}

// ---------------------------------------------------------------------------
// URL parsing & binding helpers
// ---------------------------------------------------------------------------

/// Parse a URL string into (scheme, addr) pair.
/// e.g., "https://0.0.0.0:5030" â†?("https", "0.0.0.0:5030")
fn parse_url(url: &str) -> Result<(&str, String)> {
    if let Some(rest) = url.strip_prefix("https://") {
        Ok(("https", rest.to_string()))
    } else if let Some(rest) = url.strip_prefix("http://") {
        Ok(("http", rest.to_string()))
    } else {
        Err(rust_webapp_core::error::Error::Http(format!(
            "Invalid URL '{}'. Use http://host:port or https://host:port",
            url
        )))
    }
}

/// Serve plain HTTP on the given address.
async fn serve_http(
    addr: String,
    shutdown: std::sync::Arc<tokio::sync::Notify>,
    pipeline: Arc<MiddlewarePipeline>,
    router_handler: HandlerFn,
    mode: AppMode,
    max_body_size: usize,
) {
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Failed to bind HTTP on {}: {}", addr, e);
            return;
        }
    };

    let mut join_set = JoinSet::new();

    let accept_loop = async {
        loop {
            let stream = match listener.accept().await {
                Ok((stream, _)) => stream,
                Err(e) => {
                    tracing::error!("Accept error (will retry): {}", e);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
            };

            while join_set.try_join_next().is_some() {}

            let io = TokioIo::new(stream);
            let pipeline = Arc::clone(&pipeline);
            let router_handler = router_handler.clone();

            join_set.spawn(async move {
                let svc_fn = service_fn(move |req: Request<Incoming>| {
                    let pipeline = Arc::clone(&pipeline);
                    let router_handler = router_handler.clone();
                    let mode = mode;
                    async move {
                        let start = Instant::now();
                        let method = req.method().to_string();
                        let path = req.uri().path().to_string();
                        let result =
                            handle_request(req, pipeline, router_handler, max_body_size).await;
                        let elapsed = start.elapsed();
                        if mode == AppMode::Development {
                            let status =
                                result.as_ref().map(|r| r.status().as_u16()).unwrap_or(500);
                            tracing::info!(
                                "[{}] {} â†?{} ({:.0}ms)",
                                method,
                                path,
                                status,
                                elapsed.as_secs_f64() * 1000.0
                            );
                        }
                        result
                    }
                });

                if let Err(err) = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, svc_fn)
                    .await
                {
                    tracing::error!("Connection error: {}", err);
                }
            });
        }
    };

    tokio::select! {
        _ = accept_loop => {},
        _ = shutdown.notified() => {
            tracing::info!("HTTP {}: stop accepting, draining...", addr);
            drop(listener);
        }
    }

    let drain_future = async {
        while let Some(result) = join_set.join_next().await {
            if let Err(e) = result {
                tracing::error!("Connection task panicked: {:?}", e);
            }
        }
    };

    match tokio::time::timeout(std::time::Duration::from_secs(30), drain_future).await {
        Ok(_) => tracing::info!("HTTP {}: drained.", addr),
        Err(_) => tracing::warn!("HTTP {}: drain timeout, force-terminating.", addr),
    }
}

/// Serve HTTPS (TLS) on the given address.
async fn serve_https(
    addr: String,
    acceptor: TlsAcceptor,
    shutdown: std::sync::Arc<tokio::sync::Notify>,
    pipeline: Arc<MiddlewarePipeline>,
    router_handler: HandlerFn,
    mode: AppMode,
    max_body_size: usize,
) {
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Failed to bind HTTPS on {}: {}", addr, e);
            return;
        }
    };

    let mut join_set = JoinSet::new();

    let accept_loop = async {
        loop {
            let stream = match listener.accept().await {
                Ok((stream, _)) => stream,
                Err(e) => {
                    tracing::error!("Accept error (will retry): {}", e);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
            };

            while join_set.try_join_next().is_some() {}

            let acceptor = acceptor.clone();
            let pipeline = Arc::clone(&pipeline);
            let router_handler = router_handler.clone();

            join_set.spawn(async move {
                match acceptor.accept(stream).await {
                    Ok(tls_stream) => {
                        let io = TokioIo::new(tls_stream);
                        let svc_fn = service_fn(move |req: Request<Incoming>| {
                            let pipeline = Arc::clone(&pipeline);
                            let router_handler = router_handler.clone();
                            let mode = mode;
                            async move {
                                let start = Instant::now();
                                let method = req.method().to_string();
                                let path = req.uri().path().to_string();
                                let result =
                                    handle_request(req, pipeline, router_handler, max_body_size)
                                        .await;
                                let elapsed = start.elapsed();
                                if mode == AppMode::Development {
                                    let status =
                                        result.as_ref().map(|r| r.status().as_u16()).unwrap_or(500);
                                    tracing::info!(
                                        "[{}] {} â†?{} ({:.0}ms)",
                                        method,
                                        path,
                                        status,
                                        elapsed.as_secs_f64() * 1000.0
                                    );
                                }
                                result
                            }
                        });

                        if let Err(err) = hyper::server::conn::http1::Builder::new()
                            .serve_connection(io, svc_fn)
                            .await
                        {
                            tracing::error!("TLS connection error: {}", err);
                        }
                    }
                    Err(e) => {
                        tracing::error!("TLS handshake error: {}", e);
                    }
                }
            });
        }
    };

    tokio::select! {
        _ = accept_loop => {},
        _ = shutdown.notified() => {
            tracing::info!("HTTPS {}: stop accepting, draining...", addr);
            drop(listener);
        }
    }

    let drain_future = async {
        while let Some(result) = join_set.join_next().await {
            if let Err(e) = result {
                tracing::error!("Connection task panicked: {:?}", e);
            }
        }
    };

    match tokio::time::timeout(std::time::Duration::from_secs(30), drain_future).await {
        Ok(_) => tracing::info!("HTTPS {}: drained.", addr),
        Err(_) => tracing::warn!("HTTPS {}: drain timeout, force-terminating.", addr),
    }
}

// ---------------------------------------------------------------------------
// TLS helpers
// ---------------------------------------------------------------------------

/// Build a TLS acceptor from PEM certificate and key files.
fn build_tls_acceptor(cert_path: &str, key_path: &str) -> Result<TlsAcceptor> {
    use std::fs::File;
    use std::io::BufReader;

    if cert_path.is_empty() || key_path.is_empty() {
        return Err(rust_webapp_core::error::Error::Http(
            "TLS certificate or key path not configured.".into(),
        ));
    }

    let cert_file = File::open(cert_path).map_err(|e| {
        rust_webapp_core::error::Error::Http(format!("Cannot open cert '{}': {}", cert_path, e))
    })?;
    let mut cert_reader = BufReader::new(cert_file);
    let certs: Vec<CertificateDer> = certs(&mut cert_reader).filter_map(|r| r.ok()).collect();
    if certs.is_empty() {
        return Err(rust_webapp_core::error::Error::Http(format!(
            "No valid certs in '{}'",
            cert_path
        )));
    }

    let key_file = File::open(key_path).map_err(|e| {
        rust_webapp_core::error::Error::Http(format!("Cannot open key '{}': {}", key_path, e))
    })?;
    let mut key_reader = BufReader::new(key_file);
    let key = pkcs8_private_keys(&mut key_reader)
        .filter_map(|r| r.ok())
        .map(PrivateKeyDer::from)
        .next()
        .or_else(|| {
            let key_file2 = File::open(key_path).map(BufReader::new).ok()?;
            let mut kr2 = key_file2;
            let rsa_keys: Vec<PrivateKeyDer> = rsa_private_keys(&mut kr2)
                .filter_map(|r| r.ok())
                .map(PrivateKeyDer::from)
                .collect();
            rsa_keys.into_iter().next()
        })
        .ok_or_else(|| {
            rust_webapp_core::error::Error::Http(format!("No valid private key in '{}'", key_path))
        })?;

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| rust_webapp_core::error::Error::Http(format!("TLS config error: {}", e)))?;

    Ok(TlsAcceptor::from(std::sync::Arc::new(config)))
}
