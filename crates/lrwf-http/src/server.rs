//! Host builder and hyper server integration.
//!
//! Includes built-in exception middleware: errors produced by endpoints
//! are caught and converted to well-formed HTTP error responses using
//! `Error::status_code()`.

use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::TokioIo;
use http_body_util::Full;
use lrdi::{ServiceCollection, ServiceProvider};
use lrwf_core::app::IHost;
use lrwf_core::config::{self, AppOptions};
use lrwf_core::error::Result;
use lrwf_core::http::IHttpContext;
use lrwf_core::middleware::IMiddleware;
use lrwf_core::mode::AppMode;
use lrwf_core::routing::{HttpMethod, IEndpoint, IRouter};

use crate::context::HttpContext;
use crate::cors::{CorsConfig, CorsMiddleware};
use crate::endpoint::{StaticHtmlEndpoint, StaticJsonEndpoint, StubEndpoint};
use crate::pipeline::{HandlerFn, MiddlewarePipeline};
use crate::router::Router;
use lrwf_core::di::scan::{HandlerRegistration, RouteEntry};
use lrwf_openapi::{generate_openapi_spec, APIUI_HTML};

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};

pub struct Host {
    #[allow(dead_code)]
    provider: Arc<ServiceProvider>,
    pub options: AppOptions,
    pipeline: Arc<MiddlewarePipeline>,
    router: Arc<tokio::sync::RwLock<Router>>,
    mode: AppMode,
    #[allow(dead_code)]
    spa_root: Option<String>,
}

#[allow(clippy::type_complexity)]
pub struct HostBuilder {
    service_configs: Vec<Box<dyn FnOnce(ServiceCollection) -> ServiceCollection + Send>>,
    mode: AppMode,
    spa_root: Option<String>,
    options_modifiers: Vec<Box<dyn FnOnce(&mut AppOptions) + Send>>,
    cors_config: Option<CorsConfig>,
}

#[allow(clippy::type_complexity)]
pub struct HostAppBuilder {
    options_modifiers: Vec<Box<dyn FnOnce(&mut AppOptions) + Send>>,
}

impl HostAppBuilder {
    fn new() -> Self {
        Self { options_modifiers: Vec::new() }
    }

    #[allow(non_snake_case)]
    pub fn useOptions<F>(&mut self, f: F)
    where F: FnOnce(&mut AppOptions) + Send + 'static,
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
        }
    }

    pub fn register<F>(mut self, f: F) -> Self
    where F: FnOnce(ServiceCollection) -> ServiceCollection + Send + 'static,
    {
        self.service_configs.push(Box::new(f));
        self
    }

    pub fn configure<F>(mut self, f: F) -> Self
    where F: FnOnce(&mut HostAppBuilder) + Send + 'static,
    {
        let mut builder = HostAppBuilder::new();
        f(&mut builder);
        self.options_modifiers.append(&mut builder.options_modifiers);
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

    pub fn build(self) -> Host {
        // Initialize structured logging based on app mode.
        // This is idempotent — subsequent calls are no-ops.
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

        let mut svc = ServiceCollection::new();
        for cfg in self.service_configs {
            svc = cfg(svc);
        }

        for reg in inventory::iter::<HandlerRegistration> {
            svc = (reg.register)(svc);
        }

        let provider = Arc::new(svc.build().unwrap_or_else(|e| {
            panic!("Failed to build ServiceProvider: {}. Check your DI registrations.", e);
        }));

        let mut pipeline = MiddlewarePipeline::new();
        let middlewares: Vec<Arc<dyn IMiddleware>> = provider.get_all::<dyn IMiddleware>();
        for mw in middlewares {
            pipeline.add_middleware(mw);
        }

        let appsettings = config::load_appsettings(self.mode)
            .unwrap_or_else(|| serde_json::json!({}));
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

        // Build dispatch map: handler_type → dispatch function
        #[allow(clippy::type_complexity)]
        let mut dispatch_map: std::collections::HashMap<&'static str, fn(
            Vec<u8>,
            std::collections::HashMap<String, String>,
            std::collections::HashMap<String, String>,
            Arc<ServiceProvider>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = lrwf_core::error::Result<
                lrwf_core::di::scan::ResponseData
            >> + Send>,
        >> = std::collections::HashMap::new();

        for dispatch in inventory::iter::<lrwf_core::di::scan::RouteDispatch> {
            dispatch_map.insert(dispatch.handler_type, dispatch.dispatch);
        }

        for entry in inventory::iter::<RouteEntry> {
            route_count += 1;
            let stub = Arc::new(StubEndpoint {
                method: entry.method.as_str(),
                path: entry.path,
                handler_type: entry.handler_type,
                dispatch_fn: dispatch_map.get(entry.handler_type).copied(),
                provider: Some(Arc::clone(&provider)),
            });
            router.register(entry.method, entry.path, stub);
        }

        let openapi_spec = generate_openapi_spec("LRWF API", "1.0.0");
        let openapi_bytes = serde_json::to_vec(&openapi_spec).unwrap_or_default();
        router.register(HttpMethod::Get, "/api/openapi.json",
            Arc::new(StaticJsonEndpoint { body: openapi_bytes }));
        router.register(HttpMethod::Get, "/api/openapi.html",
            Arc::new(StaticHtmlEndpoint { body: APIUI_HTML }));

        // Health check endpoints for monitoring / container orchestration
        let health_json = serde_json::to_vec(&serde_json::json!({"status":"ok"})).unwrap_or_default();
        let health_endpoint: Arc<dyn IEndpoint> = Arc::new(StaticJsonEndpoint { body: health_json });
        router.register(HttpMethod::Get, "/health", Arc::clone(&health_endpoint));
        router.register(HttpMethod::Get, "/healthz", health_endpoint);

        if self.mode == AppMode::Development {
            let version = env!("CARGO_PKG_VERSION");
            tracing::info!("");
            tracing::info!("  ─────────────────────────────────────────────────");
            tracing::info!("    Rust WebApplication Framework v{}", version);
            tracing::info!("  ─────────────────────────────────────────────────");
            tracing::info!("    App:      {}", options.app.name);
            tracing::info!("    CORS:     enabled");
            if let Some(ref root) = self.spa_root {
                tracing::info!("    SPA Root: {}", root);
            }
            if route_count > 0 {
                tracing::info!("    Routes:   {} registered", route_count);
            }
            tracing::info!("  ─────────────────────────────────────────────────");
            tracing::info!("");
        } else if route_count > 0 {
            tracing::info!("{} route(s) registered", route_count);
        }

        let router = Arc::new(tokio::sync::RwLock::new(router));

        Host { provider, options, pipeline, router, mode: self.mode, spa_root: self.spa_root }
    }
}

impl Default for HostBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Host {
    pub fn builder() -> HostBuilder { HostBuilder::new() }

    pub fn options(&self) -> &AppOptions { &self.options }

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
                other => return Err(lrwf_core::error::Error::Http(
                    format!("Unsupported URL scheme '{}' in '{}'", other, url)
                )),
            }
        }

        let acceptor = if !https_addrs.is_empty() {
            let tls = &self.options.tls;
            if tls.cert_path.is_empty() || tls.key_path.is_empty() {
                return Err(lrwf_core::error::Error::Http(
                    "HTTPS URLs require Tls.CertPath and Tls.KeyPath".into()
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
                tracing::info!("  Listening on {}{}", url,
                    if url.starts_with("https") { format!(" (OpenAPI  {}/api/openapi.html)", url) } else { String::new() });
            }
        } else {
            tracing::info!("Listening on {} url(s)", urls.len());
        }

        let notify = std::sync::Arc::new(tokio::sync::Notify::new());

        let shutdown_notify = std::sync::Arc::clone(&notify);
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                let mut sigterm = signal(SignalKind::terminate()).unwrap();
                let mut sigint = signal(SignalKind::interrupt()).unwrap();
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
        let router = Arc::clone(&self.router);
        let mode = self.mode;
        let max_body_size = self.options.app.max_body_size;

        for addr in &http_addrs {
            let addr = addr.clone();
            let n = std::sync::Arc::clone(&notify);
            let p = Arc::clone(&pipeline);
            let r = Arc::clone(&router);
            handles.push(tokio::spawn(serve_http(addr, n, p, r, mode, max_body_size)));
        }

        if let Some(ref tls_acceptor) = acceptor {
            for addr in &https_addrs {
                let addr = addr.clone();
                let n = std::sync::Arc::clone(&notify);
                let p = Arc::clone(&pipeline);
                let r = Arc::clone(&router);
                let a = tls_acceptor.clone();
                handles.push(tokio::spawn(serve_https(addr, a, n, p, r, mode, max_body_size)));
            }
        }

        for h in handles {
            let _ = h.await;
        }
        Ok(())
    }

    /// Start the server at a single explicit address (convenience wrapper).
    pub async fn run_at(&self, addr: &str) -> Result<()> {
        let notify = std::sync::Arc::new(tokio::sync::Notify::new());
        serve_http(
            addr.to_string(),
            notify,
            Arc::clone(&self.pipeline),
            Arc::clone(&self.router),
            self.mode,
            self.options.app.max_body_size,
        ).await
    }
}

#[async_trait::async_trait]
impl IHost for Host {
    async fn run(&self, addr: &str) -> Result<()> { self.run_at(addr).await }
    async fn stop(&self) -> Result<()> {
        tracing::info!("Stop requested.");
        Ok(())
    }
}

fn make_router_handler(router: Arc<tokio::sync::RwLock<Router>>) -> HandlerFn {
    Arc::new(move |ctx: &mut dyn IHttpContext| {
        let router = Arc::clone(&router);
        Box::pin(async move {
            let router = router.read().await;
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
                    write_error_response(ctx, 404, "Not Found").await;
                    Ok(())
                }
            }
        })
    })
}

async fn handle_request(
    req: Request<Incoming>,
    pipeline: Arc<MiddlewarePipeline>,
    router: Arc<tokio::sync::RwLock<Router>>,
    max_body_size: usize,
) -> std::result::Result<hyper::Response<Full<Bytes>>, std::convert::Infallible> {
    let mut ctx = HttpContext::new(req, max_body_size).await;
    let router_handler = make_router_handler(router);
    let result = pipeline.execute(&mut ctx, router_handler).await;

    if let Err(e) = result {
        let status = e.status_code();
        write_error_response(&mut ctx, status, &e.to_string()).await;
    }

    Ok(ctx.into_response())
}

async fn write_error_response(ctx: &mut dyn IHttpContext, status: u16, message: &str) {
    ctx.response_mut().set_status(status);
    ctx.response_mut().set_header("content-type", "application/json");
    let body = serde_json::json!({ "error": message, "status": status });
    let _ = ctx.response_mut()
        .write_bytes(serde_json::to_vec(&body).unwrap_or_default())
        .await;
}

// ---------------------------------------------------------------------------
// URL parsing & binding helpers
// ---------------------------------------------------------------------------

/// Parse a URL string into (scheme, addr) pair.
/// e.g., "https://0.0.0.0:5030" → ("https", "0.0.0.0:5030")
fn parse_url(url: &str) -> Result<(&str, String)> {
    if let Some(rest) = url.strip_prefix("https://") {
        Ok(("https", rest.to_string()))
    } else if let Some(rest) = url.strip_prefix("http://") {
        Ok(("http", rest.to_string()))
    } else {
        Err(lrwf_core::error::Error::Http(
            format!("Invalid URL '{}'. Use http://host:port or https://host:port", url)
        ))
    }
}

/// Serve plain HTTP on the given address.
async fn serve_http(
    addr: String,
    shutdown: std::sync::Arc<tokio::sync::Notify>,
    pipeline: Arc<MiddlewarePipeline>,
    router: Arc<tokio::sync::RwLock<Router>>,
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
            let router = Arc::clone(&router);

            join_set.spawn(async move {
                let svc_fn = service_fn(move |req: Request<Incoming>| {
                    let pipeline = Arc::clone(&pipeline);
                    let router = Arc::clone(&router);
                    let mode = mode;
                    async move {
                        let start = Instant::now();
                        let method = req.method().to_string();
                        let path = req.uri().path().to_string();
                        let result = handle_request(req, pipeline, router, max_body_size).await;
                        let elapsed = start.elapsed();
                        if mode == AppMode::Development {
                            let status = result.as_ref().map(|r| r.status().as_u16()).unwrap_or(500);
                            tracing::info!("[{}] {} → {} ({:.0}ms)", method, path, status,
                                elapsed.as_secs_f64() * 1000.0);
                        }
                        result
                    }
                });

                if let Err(err) = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, svc_fn).await
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
    router: Arc<tokio::sync::RwLock<Router>>,
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
            let router = Arc::clone(&router);

            join_set.spawn(async move {
                match acceptor.accept(stream).await {
                    Ok(tls_stream) => {
                        let io = TokioIo::new(tls_stream);
                        let svc_fn = service_fn(move |req: Request<Incoming>| {
                            let pipeline = Arc::clone(&pipeline);
                            let router = Arc::clone(&router);
                            let mode = mode;
                            async move {
                                let start = Instant::now();
                                let method = req.method().to_string();
                                let path = req.uri().path().to_string();
                                let result = handle_request(req, pipeline, router, max_body_size).await;
                                let elapsed = start.elapsed();
                                if mode == AppMode::Development {
                                    let status = result.as_ref().map(|r| r.status().as_u16()).unwrap_or(500);
                                    tracing::info!("[{}] {} → {} ({:.0}ms)", method, path, status,
                                        elapsed.as_secs_f64() * 1000.0);
                                }
                                result
                            }
                        });

                        if let Err(err) = hyper::server::conn::http1::Builder::new()
                            .serve_connection(io, svc_fn).await
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
    use std::io::BufReader;
    use std::fs::File;

    if cert_path.is_empty() || key_path.is_empty() {
        return Err(lrwf_core::error::Error::Http(
            "TLS certificate or key path not configured.".into()
        ));
    }

    let cert_file = File::open(cert_path)
        .map_err(|e| lrwf_core::error::Error::Http(format!("Cannot open cert '{}': {}", cert_path, e)))?;
    let mut cert_reader = BufReader::new(cert_file);
    let certs: Vec<CertificateDer> = certs(&mut cert_reader).into_iter().filter_map(|r| r.ok()).collect();
    if certs.is_empty() {
        return Err(lrwf_core::error::Error::Http(format!("No valid certs in '{}'", cert_path)));
    }

    let key_file = File::open(key_path)
        .map_err(|e| lrwf_core::error::Error::Http(format!("Cannot open key '{}': {}", key_path, e)))?;
    let mut key_reader = BufReader::new(key_file);
    let key = pkcs8_private_keys(&mut key_reader)
        .into_iter().filter_map(|r| r.ok()).map(PrivateKeyDer::from).next()
        .or_else(|| {
            let key_file2 = File::open(key_path).map(|f| BufReader::new(f)).ok()?;
            let mut kr2 = key_file2;
            let rsa_keys: Vec<PrivateKeyDer> = rsa_private_keys(&mut kr2)
                .into_iter().filter_map(|r| r.ok()).map(PrivateKeyDer::from).collect();
            rsa_keys.into_iter().next()
        })
        .ok_or_else(|| lrwf_core::error::Error::Http(format!("No valid private key in '{}'", key_path)))?;

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| lrwf_core::error::Error::Http(format!("TLS config error: {}", e)))?;

    Ok(TlsAcceptor::from(std::sync::Arc::new(config)))
}
