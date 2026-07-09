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
use rust_dix::{ServiceCollection, ServiceProvider};
use rust_webx_core::app::IHost;
use rust_webx_core::config::{self, AppOptions};
use rust_webx_core::error::Result;
use rust_webx_core::handler::IHostedService;
use rust_webx_core::http::IHttpContext;
use rust_webx_core::middleware::IMiddleware;
use rust_webx_core::mode::AppMode;
use rust_webx_core::routing::{HttpMethod, IEndpoint, IRouter};

use crate::auth_jwt::{init_jwt_secret, jwt_middleware, JwtAuth};
use crate::authz::collect_authorizers;
use crate::context::HttpContext;
use crate::cors::{CorsConfig, CorsMiddleware};
use crate::endpoint::{StaticHtmlEndpoint, StaticJsonEndpoint, StubEndpoint};
use crate::health::{HealthCheckEndpoint, HealthCheckRegistry, HealthStatus};
use crate::memory_cache::MemoryCache;
use crate::metrics::{HttpMetrics, MetricsEndpoint, MetricsMiddleware};
use crate::problem_response::{build_problem, write_problem_response};
use crate::rate_limit::RateLimitMiddleware;
use crate::pipeline::{HandlerFn, MiddlewarePipeline};
use crate::router::Router;
use jsonwebtoken::{DecodingKey, Validation};
use rust_webx_core::route::scan::RouteEntry;
use rust_webx_openapi::{generate_openapi_spec, APIUI_HTML};
use rust_webx_spa::SpaMiddleware;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;

/// Returns true when the JWT secret is missing, too short, or a known placeholder.
fn is_weak_jwt_secret(secret: &str) -> bool {
    secret.is_empty()
        || secret.len() < 32
        || secret.contains("change-in-production")
        || secret.contains("please-change")
        || secret.contains("demo-secret")
        || secret.contains("dev-secret")
}

pub struct Host {
    #[allow(dead_code)]
    provider: Arc<ServiceProvider>,
    pub options: AppOptions,
    pipeline: Arc<MiddlewarePipeline>,
    /// The matchit router (retained for introspection).
    #[allow(dead_code)]
    router: Arc<Router>,
    /// Pre-built router handler —eliminates per-request Arc::new.
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
    spa_disabled: bool,
    options_modifiers: Vec<Box<dyn FnOnce(&mut AppOptions) + Send>>,
    cors_config: Option<CorsConfig>,
    auth_enabled: bool,
    health_registry: Arc<HealthCheckRegistry>,
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
            // 默认从 `APP_ENV` 环境变量解析运行模式；未设置时为 Development。
            // 应用可通过 `.mode()` 显式覆盖。
            mode: AppMode::from_env(),
            spa_root: None,
            spa_disabled: false,
            options_modifiers: Vec::new(),
            cors_config: None,
            auth_enabled: false,
            health_registry: HealthCheckRegistry::new(),
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

    /// 自动绑定 appsettings 配置节到 `T`，并以 `Arc<T>` 注册到 DI 容器。
    ///
    /// 参考 ASP.NET Core 的 `services.Configure<T>(configuration.GetSection(...))`：
    /// 框架在 `build()` 时读取已合并的 appsettings（含 `{Mode}` overlay 与 env override），
    /// 取 `section` 子节点反序列化为 `T`，注册为单例供 handler 通过 `Arc<T>` 注入。
    ///
    /// ```ignore
    /// Host::builder()
    ///     .add_options::<SiteConfig>("Site")
    ///     .build()
    /// ```
    pub fn add_options<T>(self, section: &str) -> Self
    where
        T: for<'de> serde::Deserialize<'de> + Default + Send + Sync + 'static,
    {
        let mode = self.mode;
        let section = section.to_string();
        self.register(move |svc| {
            let appsettings = config::load_appsettings(mode).unwrap_or_default();
            let bound: T = config::bind_config(&appsettings, &section);
            tracing::info!(
                "[Host] add_options<{}>(\"{}\") registered",
                std::any::type_name::<T>(),
                section
            );
            svc.instance(Arc::new(bound))
        })
    }

    pub fn use_spa(mut self, root: impl Into<String>) -> Self {
        self.spa_root = Some(root.into());
        self
    }

    /// Explicitly disable SPA, including auto-detection.
    ///
    /// By default, the framework auto-detects `wwwroot/` under `app_base()`.
    /// Call this when you want a pure API host without SPA fallback,
    /// or in tests to isolate framework behavior from filesystem state.
    pub fn no_spa(mut self) -> Self {
        self.spa_disabled = true;
        self
    }

    pub fn use_cors(mut self, config: CorsConfig) -> Self {
        self.cors_config = Some(config);
        self
    }

    /// Register JWT authentication service and middleware.
    ///
    /// 参考 ASP.NET Core 的 `services.AddAuthentication()` + `app.UseAuthentication()`：
    /// 启用 JWT Bearer Token 认证，中间件自动添加到管道。授权（`#[authorize]`）
    /// 由框架通过编译期元数据自动收集，无需显式调用。
    ///
    /// ```ignore
    /// Host::builder()
    ///     .add_authentication()
    ///     .build()
    /// ```
    pub fn add_authentication(mut self) -> Self {
        self.auth_enabled = true;
        self
    }

    /// Register a memory cache instance for use via `IDistributedCache` trait.
    ///
    /// The cache is registered as a singleton in the DI container.
    /// Handlers can access it by implementing `From<Arc<MemoryCache>>`.
    ///
    /// ```ignore
    /// Host::builder()
    ///     .add_memory_cache()
    ///     .build()
    ///     .run().await;
    /// ```
    pub fn add_memory_cache(mut self) -> Self {
        let cache = Arc::new(MemoryCache::new());
        self.service_configs
            .push(Box::new(move |svc| svc.instance(cache)));
        self
    }

    /// Register a memory cache with custom configuration.
    pub fn add_memory_cache_with(mut self, max_entries: usize) -> Self {
        let cache = Arc::new(MemoryCache::new().with_max_entries(max_entries));
        self.service_configs
            .push(Box::new(move |svc| svc.instance(Arc::clone(&cache))));
        self
    }

    /// Register a middleware by type.
    ///
    /// Middleware `T` is registered as Singleton in the DI container and
    /// automatically collected by `provider.get_all::<dyn IMiddleware>()`
    /// during `build()`.
    ///
    /// ```ignore
    /// Host::builder()
    ///     .use_middleware::<RequestIdMiddleware>()
    ///     .build()
    /// ```
    pub fn use_middleware<T: IMiddleware + Default + 'static>(mut self) -> Self {
        self.service_configs.push(Box::new(|svc| {
            svc.singleton::<dyn IMiddleware>(|_| Arc::new(T::default()) as Arc<dyn IMiddleware>)
        }));
        self
    }

    /// Register a middleware with a custom factory function.
    ///
    /// Use this when the middleware requires constructor parameters:
    ///
    /// ```ignore
    /// Host::builder()
    ///     .use_middleware_with(|| {
    ///         Arc::new(RateLimitMiddleware::new(10.0, 20)) as Arc<dyn IMiddleware>
    ///     })
    ///     .build()
    /// ```
    pub fn use_middleware_with<F>(mut self, factory: F) -> Self
    where
        F: Fn() -> Arc<dyn IMiddleware> + Send + Sync + 'static,
    {
        self.service_configs.push(Box::new(move |svc| {
            svc.singleton::<dyn IMiddleware>(move |_| factory())
        }));
        self
    }

    /// Register a health check probe.
    ///
    /// Health checks are exposed via `/health` and `/health/ready` endpoints
    /// following RFC 8407 (`application/health+json` content type).
    ///
    /// ```ignore
    /// Host::builder()
    ///     .add_health_check("database", || -> HealthStatus {
    ///         if db_ping() { HealthStatus::pass() } else { HealthStatus::fail("db unreachable") }
    ///     })
    ///     .build()
    /// ```
    pub fn add_health_check<F>(self, name: impl Into<String>, check: F) -> Self
    where
        F: Fn() -> HealthStatus + Send + Sync + 'static,
    {
        self.health_registry.register(name, Arc::new(check));
        self
    }

    pub fn build(self) -> Host {
        // Initialize structured logging based on app mode.
        // This is idempotent —subsequent calls are no-ops.
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

        // Register HealthCheckRegistry as a singleton for handler injection.
        let health_registry = Arc::clone(&self.health_registry);
        svc = svc.instance(health_registry);

        let provider = svc.build().unwrap_or_else(|e| {
            panic!(
                "Failed to build ServiceProvider: {}. Check your DI registrations.",
                e
            )
        });

        // Set the global provider so #[handler] factories can resolve DI dependencies.
        rust_webx_core::route::scan::set_global_provider(Arc::clone(&provider));

        // Initialize the global handler cache from inventory registrations.
        // Handlers registered via #[handler] are collected into HandlerCache.
        // If a handler struct also has #[inject_attr], its factory will resolve
        // dependencies via the global provider set above.
        rust_webx_core::route::scan::HandlerCache::init_global();

        let appsettings =
            config::load_appsettings(self.mode).unwrap_or_else(|| serde_json::json!({}));
        let mut options: AppOptions = config::bind_root(&appsettings);
        for modifier in self.options_modifiers {
            modifier(&mut options);
        }

        let http_metrics = HttpMetrics::new();

        let mut pipeline = MiddlewarePipeline::new();

        // Default security & observability middleware (zero-config safe defaults).
        // Order: SecurityHeaders → RequestId → RateLimit → Metrics → user → CORS → SPA → Auth
        pipeline.add_middleware(Arc::new(crate::security_headers::SecurityHeadersMiddleware::new()));
        pipeline.add_middleware(Arc::new(crate::request_id::RequestIdMiddleware::new()));

        if options.rate_limit.enabled {
            tracing::info!(
                "[Host] RateLimit enabled: {} req/s, burst {}, max_ips {}",
                options.rate_limit.requests_per_second,
                options.rate_limit.burst_size,
                options.rate_limit.max_tracked_ips
            );
            pipeline.add_middleware(Arc::new(RateLimitMiddleware::from_config(
                &options.rate_limit,
            )));
        }

        if options.metrics.enabled {
            tracing::info!("[Host] Metrics enabled at GET /metrics");
            pipeline.add_middleware(Arc::new(MetricsMiddleware::new(Arc::clone(
                &http_metrics,
            ))));
        }

        let middlewares: Vec<Arc<dyn IMiddleware>> = provider.get_all::<dyn IMiddleware>();
        for mw in middlewares {
            pipeline.add_middleware(mw);
        }

        let cors = self.cors_config.unwrap_or_else(|| {
            let cs = &options.cors;
            CorsConfig {
                origins: cs.origins.clone(),
                methods: cs.methods.clone(),
                headers: cs.headers.clone(),
                expose_headers: cs.expose_headers.clone(),
                allow_credentials: cs.allow_credentials,
                max_age: cs.max_age,
            }
        });
        pipeline.add_middleware(Arc::new(CorsMiddleware::new(cors)));

        // SPA 自动启用：若应用显式调用 `use_spa`，使用指定根目录；
        // 否则框架自动检测应用基准目录下的 `wwwroot/`，存在即启用 SPA。
        // 这样新应用无需在 main.rs 手写 `use_spa("wwwroot")` 样板。
        // `no_spa()` 可显式禁用此行为（含自动检测），用于纯 API 主机或测试隔离。
        let spa_root = if self.spa_disabled {
            None
        } else {
            self.spa_root.clone().or_else(|| {
                let candidate = rust_webx_core::paths::app_base().join("wwwroot");
                if candidate.is_dir() {
                    tracing::info!("[Host] Auto-detected SPA root: {}", candidate.display());
                    Some(candidate.to_string_lossy().into_owned())
                } else {
                    None
                }
            })
        };
        if let Some(ref spa_root) = spa_root {
            pipeline.add_middleware(Arc::new(SpaMiddleware::new(spa_root.clone())));
        }

        if self.auth_enabled {
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
                    "add_authentication() enabled but no JWT secret configured. Set Jwt.Secret in appsettings.json, JWT_SECRET, or APP__Jwt__Secret env var."
                );
            }
        }

        // Users can register additional middleware here, e.g.:
        // pipeline.add_middleware(Arc::new(RequestIdMiddleware::new()));
        // pipeline.add_middleware(Arc::new(TimingMiddleware::new()));

        let pipeline = Arc::new(pipeline);

        // Security: fail fast in production when auth is enabled but JWT secret is missing or weak
        if self.mode == AppMode::Production && self.auth_enabled && is_weak_jwt_secret(&options.jwt.secret)
        {
            panic!(
                "Production requires a strong JWT secret. Set APP__Jwt__Secret or JWT_SECRET env var (min 32 chars, not a placeholder)."
            );
        }

        // Security: fail fast in production when CORS allows all origins
        if self.mode == AppMode::Production && options.cors.origins.iter().any(|o| o == "*") {
            panic!(
                "Production forbids CORS origin '*'. Set explicit origins via Cors.Origins or APP__Cors__Origins."
            );
        }

        let mut router = Router::new();
        let mut route_count = 0usize;

        // Build dispatch map: handler_type →dispatch function
        #[allow(clippy::type_complexity)]
        let mut dispatch_map: std::collections::HashMap<
            &'static str,
            fn(
                Vec<u8>,
                std::collections::HashMap<String, String>,
                std::collections::HashMap<String, String>,
                Option<Box<dyn rust_webx_core::auth::IClaims>>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = rust_webx_core::error::Result<rust_webx_core::route::scan::ResponseData>,
                        > + Send,
                >,
            >,
        > = std::collections::HashMap::new();

        for dispatch in inventory::iter::<rust_webx_core::route::scan::RouteDispatch> {
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

        let openapi_spec = generate_openapi_spec("Rust WebX API", env!("CARGO_PKG_VERSION"));
        if self.mode == AppMode::Development {
            let openapi_bytes = serde_json::to_vec(&openapi_spec).unwrap_or_default();
            router.register(
                HttpMethod::Get,
                "/api/openapi.json",
                Arc::new(StaticJsonEndpoint::new(openapi_bytes)),
            );
            router.register(
                HttpMethod::Get,
                "/api/openapi.html",
                Arc::new(StaticHtmlEndpoint { body: APIUI_HTML }),
            );
        }

        // Health check endpoints (RFC 8407 application/health+json)
        // Probes are evaluated on each request (not baked at build time).
        let health_registry = Arc::clone(&self.health_registry);
        let health_endpoint: Arc<dyn IEndpoint> =
            Arc::new(HealthCheckEndpoint::new(Arc::clone(&health_registry)));
        router.register(HttpMethod::Get, "/health", Arc::clone(&health_endpoint));
        router.register(HttpMethod::Get, "/healthz", health_endpoint);

        // /health/live: liveness (process alive → pass, no probe queries)
        let live_body = serde_json::to_vec(&serde_json::json!({"status":"pass"}))
            .unwrap_or_default();
        router.register(
            HttpMethod::Get,
            "/health/live",
            Arc::new(StaticJsonEndpoint {
                body: live_body,
                content_type: "application/health+json",
            }),
        );

        // /health/ready: readiness (queries registry, same as /health)
        router.register(
            HttpMethod::Get,
            "/health/ready",
            Arc::new(HealthCheckEndpoint::new(health_registry)),
        );

        if options.metrics.enabled {
            router.register(
                HttpMethod::Get,
                "/metrics",
                Arc::new(MetricsEndpoint::new(http_metrics)),
            );
        }

        if self.mode == AppMode::Development {
            let version = env!("CARGO_PKG_VERSION");
            tracing::info!("");
            tracing::info!("  ----------------------------------------------------------------");
            tracing::info!("    Rust WebX Framework v{}", version);
            tracing::info!("  ----------------------------------------------------------------");
            tracing::info!("    App:      {}", options.app.name);
            tracing::info!("    CORS:     enabled");
            if let Some(ref root) = self.spa_root {
                tracing::info!("    SPA Root: {}", root);
            }
            if route_count > 0 {
                tracing::info!("    Routes:   {} registered", route_count);
            }
            crate::diagnostics::log_startup_diagnostics();
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
            tracing::info!("  ----------------------------------------------------------------");
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
        start_hosted_services(&self.hosted_services).await?;

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
                    return Err(rust_webx_core::error::Error::Http(format!(
                        "Unsupported URL scheme '{}' in '{}'",
                        other, url
                    )))
                }
            }
        }

        let acceptor = if !https_addrs.is_empty() {
            let tls = &self.options.tls;
            if tls.cert_path.is_empty() || tls.key_path.is_empty() {
                return Err(rust_webx_core::error::Error::Http(
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
        install_shutdown_handler(Arc::clone(&notify));
        spawn_async_shutdown_listener(Arc::clone(&notify));

        let mut handles = Vec::new();
        let pipeline = Arc::clone(&self.pipeline);
        let router_handler = self.router_handler.clone();
        let mode = self.mode;
        let max_body_size = self.options.app.max_body_size;
        let max_connections = self.options.app.max_connections;

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
                max_connections,
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
                    max_connections,
                )));
            }
        }

        // Wait for all HTTP listeners to finish (they exit after shutdown signal).
        for h in handles {
            let _ = h.await;
        }

        stop_hosted_services(&self.hosted_services).await;

        Ok(())
    }

    /// Start the server at a single explicit address (convenience wrapper).
    pub async fn run_at(&self, addr: &str) -> Result<()> {
        start_hosted_services(&self.hosted_services).await?;

        let notify = Arc::clone(&self.shutdown);
        install_shutdown_handler(Arc::clone(&notify));
        spawn_async_shutdown_listener(Arc::clone(&notify));
        serve_http(
            addr.to_string(),
            notify,
            Arc::clone(&self.pipeline),
            self.router_handler.clone(),
            self.mode,
            self.options.app.max_body_size,
            self.options.app.max_connections,
        )
        .await;

        stop_hosted_services(&self.hosted_services).await;
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
        self.shutdown.notify_waiters();
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
                    let path_exists = router.path_exists(ctx.request().path());
                    drop(router);
                    // Don't overwrite if a middleware (e.g. SPA) already
                    // wrote a response body (static file, index.html fallback).
                    if !ctx.response().has_body() {
                        if path_exists {
                            write_error_response(ctx, 405, "Method Not Allowed").await;
                        } else {
                            write_error_response(ctx, 404, "Not Found").await;
                        }
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

    // If HttpContext::new already set an error response (e.g. 413 Payload Too Large),
    // skip the pipeline — the response is final.
    if ctx.response().status() < 400 {
        let result = pipeline.execute(&mut ctx, router_handler).await;
        if let Err(e) = result {
            let status = e.status_code();
            write_error_response(&mut ctx, status, &e.to_string()).await;
        }
    }

    Ok(ctx.into_response())
}

async fn write_error_response(ctx: &mut dyn IHttpContext, status: u16, message: &str) {
    write_problem_response(ctx, build_problem(status, message)).await;
}

// ---------------------------------------------------------------------------
// Shutdown helpers
// ---------------------------------------------------------------------------

/// Register a process-wide Ctrl+C / SIGINT handler.
///
/// On Windows, `cargo run` may exit the parent without stopping the child;
/// registering here ensures the server process itself receives the signal.
/// A second Ctrl+C forces immediate exit.
fn install_shutdown_handler(shutdown: Arc<tokio::sync::Notify>) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CTRL_COUNT: AtomicUsize = AtomicUsize::new(0);

    if let Err(e) = ctrlc::set_handler(move || {
        let n = CTRL_COUNT.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            tracing::info!("Shutdown signal received, draining connections...");
            shutdown.notify_waiters();
        } else {
            tracing::warn!("Second interrupt — force exiting.");
            std::process::exit(130);
        }
    }) {
        tracing::warn!("Could not install Ctrl+C handler: {}", e);
    }
}

/// Tokio-based listener for Ctrl+C (all platforms) and SIGTERM (Unix).
fn spawn_async_shutdown_listener(shutdown: Arc<tokio::sync::Notify>) {
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        tracing::info!("Async shutdown signal received, draining connections...");
        shutdown.notify_waiters();
    });
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            res = tokio::signal::ctrl_c() => {
                if let Err(e) = res {
                    tracing::warn!("Ctrl+C listener error: {}", e);
                }
            }
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!("Ctrl+C listener error: {}", e);
        }
    }
}

async fn start_hosted_services(services: &[Arc<dyn IHostedService>]) -> Result<()> {
    if services.is_empty() {
        return Ok(());
    }
    tracing::info!("Starting {} hosted service(s)...", services.len());
    for svc in services {
        svc.start().await?;
    }
    tracing::info!("All hosted services started.");
    Ok(())
}

async fn stop_hosted_services(services: &[Arc<dyn IHostedService>]) {
    if services.is_empty() {
        return;
    }
    tracing::info!("Stopping {} hosted service(s)...", services.len());
    for svc in services {
        if let Err(e) = svc.stop().await {
            tracing::warn!("Hosted service stop error: {}", e);
        }
    }
    tracing::info!("All hosted services stopped.");
}

async fn drain_connections(
    join_set: &mut JoinSet<()>,
    label: &str,
    mode: AppMode,
) {
    let timeout_secs = if mode == AppMode::Development { 5 } else { 30 };
    let drain_future = async {
        while let Some(result) = join_set.join_next().await {
            if let Err(e) = result {
                tracing::error!("Connection task panicked: {:?}", e);
            }
        }
    };

    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), drain_future).await {
        Ok(_) => tracing::info!("{}: drained.", label),
        Err(_) => {
            tracing::warn!("{}: drain timeout, aborting connections.", label);
            join_set.abort_all();
            while join_set.join_next().await.is_some() {}
        }
    }
}

// ---------------------------------------------------------------------------
// URL parsing & binding helpers
// ---------------------------------------------------------------------------

/// Parse a URL string into (scheme, addr) pair.
/// e.g., "https://0.0.0.0:5030" →("https", "0.0.0.0:5030")
fn parse_url(url: &str) -> Result<(&str, String)> {
    if let Some(rest) = url.strip_prefix("https://") {
        Ok(("https", rest.to_string()))
    } else if let Some(rest) = url.strip_prefix("http://") {
        Ok(("http", rest.to_string()))
    } else {
        Err(rust_webx_core::error::Error::Http(format!(
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
    max_connections: usize,
) {
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Failed to bind HTTP on {}: {}", addr, e);
            return;
        }
    };

    let sem = Arc::new(tokio::sync::Semaphore::new(max_connections));
    let mut join_set = JoinSet::new();

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                let (stream, _) = match accept_result {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!("Accept error (will retry): {}", e);
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        continue;
                    }
                };

                while join_set.try_join_next().is_some() {}

                let permit = match sem.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                let io = TokioIo::new(stream);
                let pipeline = Arc::clone(&pipeline);
                let router_handler = router_handler.clone();

                join_set.spawn(async move {
                    let _permit = permit;
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
                                    "[{}] {} -> {} ({:.0}ms)",
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
            _ = shutdown.notified() => {
                tracing::info!("HTTP {}: stop accepting, draining...", addr);
                break;
            }
        }
    }

    drain_connections(&mut join_set, &format!("HTTP {}", addr), mode).await;
}

/// Serve HTTPS (TLS) on the given address.
#[allow(clippy::too_many_arguments)]
async fn serve_https(
    addr: String,
    acceptor: TlsAcceptor,
    shutdown: std::sync::Arc<tokio::sync::Notify>,
    pipeline: Arc<MiddlewarePipeline>,
    router_handler: HandlerFn,
    mode: AppMode,
    max_body_size: usize,
    max_connections: usize,
) {
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Failed to bind HTTPS on {}: {}", addr, e);
            return;
        }
    };

    let sem = Arc::new(tokio::sync::Semaphore::new(max_connections));
    let mut join_set = JoinSet::new();

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                let (stream, _) = match accept_result {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!("Accept error (will retry): {}", e);
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        continue;
                    }
                };

                while join_set.try_join_next().is_some() {}

                let permit = match sem.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                let acceptor = acceptor.clone();
                let pipeline = Arc::clone(&pipeline);
                let router_handler = router_handler.clone();

                join_set.spawn(async move {
                    let _permit = permit;
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
                                            "[{}] {} -> {} ({:.0}ms)",
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
            _ = shutdown.notified() => {
                tracing::info!("HTTPS {}: stop accepting, draining...", addr);
                break;
            }
        }
    }

    drain_connections(&mut join_set, &format!("HTTPS {}", addr), mode).await;
}

// ---------------------------------------------------------------------------
// TLS helpers
// ---------------------------------------------------------------------------

/// Build a TLS acceptor from PEM certificate and key files.
fn build_tls_acceptor(cert_path: &str, key_path: &str) -> Result<TlsAcceptor> {
    use std::fs::File;
    use std::io::BufReader;

    if cert_path.is_empty() || key_path.is_empty() {
        return Err(rust_webx_core::error::Error::Http(
            "TLS certificate or key path not configured.".into(),
        ));
    }

    let cert_file = File::open(cert_path).map_err(|e| {
        rust_webx_core::error::Error::Http(format!("Cannot open cert '{}': {}", cert_path, e))
    })?;
    let mut cert_reader = BufReader::new(cert_file);
    let certs: Vec<CertificateDer> = certs(&mut cert_reader).filter_map(|r| r.ok()).collect();
    if certs.is_empty() {
        return Err(rust_webx_core::error::Error::Http(format!(
            "No valid certs in '{}'",
            cert_path
        )));
    }

    let key_file = File::open(key_path).map_err(|e| {
        rust_webx_core::error::Error::Http(format!("Cannot open key '{}': {}", key_path, e))
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
            rust_webx_core::error::Error::Http(format!("No valid private key in '{}'", key_path))
        })?;

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| rust_webx_core::error::Error::Http(format!("TLS config error: {}", e)))?;

    Ok(TlsAcceptor::from(std::sync::Arc::new(config)))
}
