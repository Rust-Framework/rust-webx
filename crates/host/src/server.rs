//! Host builder and hyper server integration.
//!
//! Includes built-in exception middleware: errors produced by endpoints
//! are caught and converted to well-formed HTTP error responses using
//! `Error::status_code()`.

use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::TokioIo;
use rust_dix::{ServiceCollection, ServiceProvider};
use webx_core::app::IHost;
use webx_core::config::{self, AppOptions};
use webx_core::error::Result;
use webx_core::handler::IHostedService;
use webx_core::http::IHttpContext;
use webx_core::localization::{IStringLocalizer, LocalizationOptions, StringLocalizer};
use webx_core::middleware::IMiddleware;
use webx_core::mode::AppMode;
use webx_core::route::catalog::{EndpointCatalog, IEndpointCatalog};
use webx_core::routing::{HttpMethod, IEndpoint, IRouter};

use crate::auth_jwt::{init_jwt_secret, jwt_middleware, JwtAuth};
use crate::authz::collect_authorizers;
use crate::context::{BodyLimits, HttpContext, RespBody};
use crate::cors::{CorsConfig, CorsMiddleware};
use crate::endpoint::{StaticHtmlEndpoint, StaticJsonEndpoint, StubEndpoint};
use crate::health::{HealthCheckEndpoint, HealthCheckRegistry, HealthStatus};
use crate::memory_cache::MemoryCache;
use crate::metrics::{HttpMetrics, MetricsEndpoint, MetricsMiddleware};
use crate::pipeline::{HandlerFn, MiddlewarePipeline};
use crate::problem_response::{build_problem, write_problem_response, write_unauthorized};
use crate::rate_limit::RateLimitMiddleware;
use crate::router::Router;
use jsonwebtoken::{DecodingKey, Validation};
use webx_core::route::scan::RouteEntry;
use webx_core::DispatchRuntime;
use webx_openapi::{generate_openapi_spec, APIUI_HTML};
use webx_spa::{EmbeddedAssets, SpaMiddleware, SpaSource};

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

/// Log where the SPA files come from and which embedded files are overridden.
///
/// Overrides are the point of the feature, but a stale one silently defeats an
/// updated build, so the list is logged instead of waiting to be discovered.
fn log_spa_source(spa: &SpaMiddleware) {
    tracing::info!("[Host] SPA root: {}", spa.resolved_root().display());
    let Some(assets) = spa.embedded() else {
        return;
    };
    tracing::info!(
        "[Host] Embedded assets: {} files, {:.1} MiB embedded ({:.1} MiB uncompressed), built from `{}`",
        assets.len(),
        assets.total_bytes() as f64 / (1024.0 * 1024.0),
        assets.total_raw_bytes() as f64 / (1024.0 * 1024.0),
        assets.root(),
    );
    let shadowed = assets.shadowed_by(spa.resolved_root());
    if !shadowed.is_empty() {
        tracing::warn!(
            "[Host] {} embedded file(s) are overridden on disk: {}. \
             An override older than the current build stays in effect until it is \
             updated or deleted.",
            shadowed.len(),
            shadowed.join(", "),
        );
    }
}

/// Layer a compiled-in table under `source` when one was registered.
///
/// Opt-in is `#[webx::main(embed)]` (plus `build.rs` `web_root`).
/// `use_spa` names the disk overlay; without it, the table's build-time root
/// is used as the overlay path.
fn with_embedded_assets(
    source: Option<SpaSource>,
    assets: Option<EmbeddedAssets>,
) -> Option<SpaSource> {
    let Some(assets) = assets else {
        return source;
    };
    let overlay_root = match source.as_ref() {
        Some(existing) => existing.disk_root().to_string(),
        None => assets.root().to_string(),
    };
    Some(SpaSource::Embedded {
        assets,
        overlay_root,
    })
}

/// Shutdown signal shared by the host, its handle and every listener.
///
/// `watch` (not `Notify`) because a signal must not be lost: `Notify::notify_waiters`
/// drops the wakeup when no listener is parked yet, which is exactly the case in
/// the documented "spawn(run) then handle.shutdown()" pattern. A `watch` keeps
/// the `true` value, so a listener that subscribes later still sees it.
type ShutdownSignal = Arc<tokio::sync::watch::Sender<bool>>;

pub struct Host {
    #[allow(dead_code)]
    provider: Arc<ServiceProvider>,
    dispatch_runtime: Arc<DispatchRuntime>,
    pub options: AppOptions,
    /// Endpoint metadata for this host, built once from the compile-time routes.
    endpoints: Arc<dyn IEndpointCatalog>,
    /// Resolves endpoint resource keys for the host's configured culture.
    localizer: Arc<dyn IStringLocalizer>,
    /// Body and upload limits, derived from `App` + `Form` configuration.
    body_limits: BodyLimits,
    pipeline: Arc<MiddlewarePipeline>,
    /// The matchit router (retained for introspection).
    #[allow(dead_code)]
    router: Arc<Router>,
    /// Pre-built router handler —eliminates per-request Arc::new.
    router_handler: HandlerFn,
    mode: AppMode,
    /// Kept for introspection; the middleware already holds its own copy.
    #[allow(dead_code)]
    spa_source: Option<SpaSource>,
    shutdown: ShutdownSignal,
    /// Hosted services that are started on host start
    /// and stopped on graceful shutdown.
    hosted_services: Vec<Arc<dyn IHostedService>>,
}

/// Factory for a user-registered middleware.
type MiddlewareFactory = Box<dyn Fn() -> Arc<dyn IMiddleware> + Send + Sync>;

/// A user middleware waiting to be placed in the pipeline at its slot.
struct MiddlewareSpec {
    slot: webx_core::middleware::MiddlewareSlot,
    factory: MiddlewareFactory,
}

#[allow(clippy::type_complexity)]
pub struct HostBuilder {
    service_configs: Vec<Box<dyn FnOnce(ServiceCollection) -> ServiceCollection + Send>>,
    mode: AppMode,
    spa_source: Option<SpaSource>,
    spa_disabled: bool,
    options_modifiers: Vec<Box<dyn FnOnce(&mut AppOptions) + Send>>,
    cors_config: Option<CorsConfig>,
    auth_enabled: bool,
    localization: Option<LocalizationOptions>,
    health_registry: Arc<HealthCheckRegistry>,
    /// Middlewares registered through the builder, kept with their slot so
    /// ordering does not depend on DI enumeration order.
    middleware_specs: Vec<MiddlewareSpec>,
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
            // 默认从 `APP_ENV` 环境变量解析运行模式；未设置时为 Production。
            // 应用可通过 `.mode()` 显式覆盖（本机开发通常设 APP_ENV=Development）。
            mode: AppMode::from_env(),
            spa_source: None,
            spa_disabled: false,
            options_modifiers: Vec::new(),
            cors_config: None,
            auth_enabled: false,
            localization: None,
            health_registry: HealthCheckRegistry::new(),
            middleware_specs: Vec::new(),
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

    /// Serve SPA static files from a directory on disk.
    ///
    /// ```ignore
    /// Host::builder().use_spa("wwwroot").build()
    /// ```
    ///
    /// **Runtime disk root** — the directory next to the process (or under
    /// `app_base`) where operators may drop overrides. This is not
    /// `build.rs` `web_root` (that path only names what was baked in).
    ///
    /// When the binary was built with `#[webx::main(embed)]` (and
    /// `build.rs` wrote a table), that table is layered underneath this
    /// directory at [`build`](Self::build): same-named disk files win.
    /// Without `(embed)`, only the disk directory is served.
    pub fn use_spa(mut self, source: impl Into<SpaSource>) -> Self {
        self.spa_source = Some(match (self.spa_source.take(), source.into()) {
            // Compiled-in files are already configured: only move the directory
            // overrides are read from, rather than dropping the baseline.
            (Some(SpaSource::Embedded { assets, .. }), SpaSource::Disk(root)) => {
                SpaSource::Embedded {
                    assets,
                    overlay_root: root,
                }
            }
            (_, incoming) => incoming,
        });
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

    /// Register the JWT authentication service and middleware.
    ///
    /// The ASP.NET Core `AddAuthentication()` + `UseAuthentication()` pair: it
    /// enables JWT bearer authentication so endpoints that declare
    /// `#[authorize]` receive claims. Route authorization itself is part of each
    /// endpoint's compile-time metadata and is always enforced after routing —
    /// there is no second switch to turn it on.
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

    /// Enable localization with the conventional resource layout.
    ///
    /// Reads `<culture>.json` files from `locales/` under the application base
    /// (for example `locales/zh-CN.json`, `locales/en.json`). The active culture
    /// is `Culture.Default` from appsettings (or `APP__Culture__Default`); it
    /// decides the language of endpoint names, OpenAPI, and the endpoint
    /// catalog.
    ///
    /// ```ignore
    /// Host::builder().add_localization().build()
    /// ```
    ///
    /// A missing `locales/` directory is not an error; keys then fall back to
    /// their literal or the key itself. For a single-file deployment, register
    /// compiled-in resources with [`Self::add_localization_with`].
    pub fn add_localization(mut self) -> Self {
        self.localization = Some(LocalizationOptions::new());
        self
    }

    /// Enable localization with explicit options.
    ///
    /// Use this for compiled-in resources (`include_str!`) or a non-conventional
    /// resource directory. Explicit resources override files with the same
    /// culture and key.
    ///
    /// ```ignore
    /// Host::builder()
    ///     .add_localization_with(
    ///         LocalizationOptions::none()
    ///             .resources("zh-CN", include_str!("../locales/zh-CN.json"))
    ///             .resources("en", include_str!("../locales/en.json")),
    ///     )
    ///     .build()
    /// ```
    pub fn add_localization_with(mut self, options: LocalizationOptions) -> Self {
        self.localization = Some(options);
        self
    }

    /// Deprecated: route authorization is enforced from endpoint metadata.
    ///
    /// Every endpoint that declares `#[authorize]` is checked by the endpoint
    /// itself, after routing; there is no separate policy switch. This method
    /// does nothing and will be removed in a future major release.
    #[deprecated(
        since = "0.6.0",
        note = "Route authorization is enforced from endpoint metadata; remove this call"
    )]
    pub fn use_resource_authorization(self) -> Self {
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

    /// Register a middleware by type in the default slot
    /// ([`MiddlewareSlot::AfterAuth`](webx_core::middleware::MiddlewareSlot::AfterAuth)).
    ///
    /// The default slot runs **after** authentication, so middleware that reads
    /// `ctx.claims()` actually sees the principal. Use
    /// [`Self::use_middleware_at`] to choose a different slot.
    ///
    /// ```ignore
    /// Host::builder()
    ///     .use_middleware::<TimingMiddleware>()
    ///     .build()
    /// ```
    pub fn use_middleware<T: IMiddleware + Default + 'static>(self) -> Self {
        self.use_middleware_at::<T>(webx_core::middleware::MiddlewareSlot::AfterAuth)
    }

    /// Register a middleware by type in a specific pipeline slot.
    pub fn use_middleware_at<T: IMiddleware + Default + 'static>(
        mut self,
        slot: webx_core::middleware::MiddlewareSlot,
    ) -> Self {
        self.middleware_specs.push(MiddlewareSpec {
            slot,
            factory: Box::new(|| Arc::new(T::default()) as Arc<dyn IMiddleware>),
        });
        self
    }

    /// Register a middleware with a custom factory function (default slot:
    /// [`MiddlewareSlot::AfterAuth`](webx_core::middleware::MiddlewareSlot::AfterAuth)).
    ///
    /// Use this when the middleware requires constructor parameters:
    ///
    /// ```ignore
    /// Host::builder()
    ///     .use_middleware_with(|| {
    ///         Arc::new(TimingMiddleware::new("api")) as Arc<dyn IMiddleware>
    ///     })
    ///     .build()
    /// ```
    pub fn use_middleware_with<F>(self, factory: F) -> Self
    where
        F: Fn() -> Arc<dyn IMiddleware> + Send + Sync + 'static,
    {
        self.use_middleware_with_at(webx_core::middleware::MiddlewareSlot::AfterAuth, factory)
    }

    /// Register a middleware with a custom factory in a specific pipeline slot.
    pub fn use_middleware_with_at<F>(
        mut self,
        slot: webx_core::middleware::MiddlewareSlot,
        factory: F,
    ) -> Self
    where
        F: Fn() -> Arc<dyn IMiddleware> + Send + Sync + 'static,
    {
        self.middleware_specs.push(MiddlewareSpec {
            slot,
            factory: Box::new(factory),
        });
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

        // Load configuration first: the localizer and endpoint catalog need the
        // host-wide culture, and both are registered in DI.
        let appsettings =
            config::load_appsettings(self.mode).unwrap_or_else(|| serde_json::json!({}));
        let mut options: AppOptions = config::bind_root(&appsettings);
        for modifier in self.options_modifiers {
            modifier(&mut options);
        }

        let default_culture = if options.culture.default.trim().is_empty() {
            "en".to_string()
        } else {
            options.culture.default.clone()
        };
        let localization = self.localization.unwrap_or_else(LocalizationOptions::none);
        let localizer: Arc<dyn IStringLocalizer> = Arc::new(
            StringLocalizer::new(default_culture, &localization)
                .unwrap_or_else(|error| panic!("Invalid localization resources: {error}")),
        );
        let endpoints: Arc<dyn IEndpointCatalog> = Arc::new(EndpointCatalog::from_inventory());

        let mut svc = ServiceCollection::from_injected();
        for cfg in self.service_configs {
            svc = cfg(svc);
        }

        // Register HealthCheckRegistry as a singleton for handler injection.
        let health_registry = Arc::clone(&self.health_registry);
        svc = svc.instance(health_registry);

        // Endpoint metadata and localization are host services, not globals.
        let endpoints_for_di = Arc::clone(&endpoints);
        svc = svc.singleton::<dyn IEndpointCatalog>(move |_| Arc::clone(&endpoints_for_di));
        let localizer_for_di = Arc::clone(&localizer);
        svc = svc.singleton::<dyn IStringLocalizer>(move |_| Arc::clone(&localizer_for_di));

        let provider = svc.build().unwrap_or_else(|e| {
            panic!(
                "Failed to build ServiceProvider: {}. Check your DI registrations.",
                e
            )
        });

        // Per-host dispatch runtime (provider + handler registry).
        let handler_cache = Arc::new(webx_core::route::scan::HandlerCache::build());
        let dispatch_runtime = Arc::new(DispatchRuntime::new(Arc::clone(&provider), handler_cache));

        // Upload spooling and body limits come from the same merged config.
        let body_limits = BodyLimits::from_options(&options);
        body_limits.apply_spool_root();
        tracing::info!(
            "[Host] Uploads: max request {} bytes, max file {} bytes, spool >{} bytes to {}",
            body_limits.multipart_body,
            body_limits.file,
            body_limits.memory_threshold,
            body_limits.spool_dir.display()
        );

        let http_metrics = HttpMetrics::new();

        let mut pipeline = MiddlewarePipeline::new();

        // Pipeline order (see docs/rust-webx/07-middleware/ordering-strategy.md):
        //
        //   SecurityHeaders → RequestId → user(BeforeCors) → CORS → Metrics →
        //   RateLimit → auth → user(AfterAuth, default) → SPA → user(AfterSpa) → Router
        //
        // CORS runs before the short-circuiting middlewares so a 429/413/401
        // still carries Access-Control-Allow-Origin; Metrics runs before
        // RateLimit so rate-limited requests are counted; user middleware
        // defaults to AfterAuth so it can read the authenticated principal.
        pipeline.add_middleware(Arc::new(
            crate::security_headers::SecurityHeadersMiddleware::new(),
        ));
        pipeline.add_middleware(Arc::new(crate::request_id::RequestIdMiddleware::new()));

        for spec in self
            .middleware_specs
            .iter()
            .filter(|spec| spec.slot == webx_core::middleware::MiddlewareSlot::BeforeCors)
        {
            pipeline.add_middleware((spec.factory)());
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

        if options.metrics.enabled {
            tracing::info!("[Host] Metrics enabled at GET /metrics");
            pipeline.add_middleware(Arc::new(MetricsMiddleware::new(Arc::clone(&http_metrics))));
        }

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

        // User middlewares in the default slot: after authentication, so
        // `ctx.claims()` is populated. Middlewares registered directly on the
        // DI container join the same slot, after the builder-registered ones.
        for spec in self
            .middleware_specs
            .iter()
            .filter(|spec| spec.slot == webx_core::middleware::MiddlewareSlot::AfterAuth)
        {
            pipeline.add_middleware((spec.factory)());
        }
        for mw in provider.get_all::<dyn IMiddleware>() {
            pipeline.add_middleware(mw);
        }

        // SPA 自动启用：若应用显式调用 `use_spa`，使用指定的文件源；
        // 否则框架自动检测应用基准目录下的 `wwwroot/`，存在即启用 SPA。
        // 这样新应用无需在 main.rs 手写 `use_spa("wwwroot")` 样板。
        // `no_spa()` 可显式禁用此行为（含自动检测），用于纯 API 主机或测试隔离。
        // A registered `embed_assets!()` table is layered underneath automatically.
        // SPA runs after JWT so auth middleware sees API requests first; SpaMiddleware
        // skips /api/* paths so unmatched API routes return 404 from the router.
        let spa_source = if self.spa_disabled {
            None
        } else {
            let base = self.spa_source.clone().or_else(|| {
                let candidate = webx_core::paths::app_base().join("wwwroot");
                if candidate.is_dir() {
                    tracing::info!("[Host] Auto-detected SPA root: {}", candidate.display());
                    Some(SpaSource::Disk(candidate.to_string_lossy().into_owned()))
                } else {
                    None
                }
            });
            with_embedded_assets(base, EmbeddedAssets::registered().copied())
        };
        let mut spa_fallback: Option<Arc<SpaMiddleware>> = None;
        if let Some(ref source) = spa_source {
            let spa = Arc::new(SpaMiddleware::from_source(source.clone()));
            log_spa_source(&spa);
            pipeline.add_middleware(Arc::clone(&spa) as Arc<dyn IMiddleware>);
            // Kept for the post-routing index fallback: static files are served
            // before routing, but the SPA shell is only served when the router
            // has no match.
            spa_fallback = Some(spa);
        }

        // Last-chance hooks: after static files/SPA, immediately before routing.
        for spec in self
            .middleware_specs
            .iter()
            .filter(|spec| spec.slot == webx_core::middleware::MiddlewareSlot::AfterSpa)
        {
            pipeline.add_middleware((spec.factory)());
        }

        let pipeline = Arc::new(pipeline);

        // Security: fail fast in production when auth is enabled but JWT secret is missing or weak
        if self.mode == AppMode::Production
            && self.auth_enabled
            && is_weak_jwt_secret(&options.jwt.secret)
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

        crate::diagnostics::assert_route_configuration_valid();

        // Build dispatch map: handler_type → dispatch function
        let mut dispatch_map: std::collections::HashMap<
            &'static str,
            webx_core::route::scan::RouteDispatchFn,
        > = std::collections::HashMap::new();

        for dispatch in inventory::iter::<webx_core::route::scan::RouteDispatch> {
            dispatch_map.insert(dispatch.handler_type, dispatch.dispatch);
        }

        for entry in inventory::iter::<RouteEntry> {
            route_count += 1;
            let stub = Arc::new(StubEndpoint {
                method: entry.method.as_str(),
                path: entry.path,
                handler_type: entry.handler_type,
                dispatch_fn: dispatch_map.get(entry.handler_type).copied(),
                authorization: entry.authorization,
                authorizers: collect_authorizers(provider.as_ref()),
            });
            router.register(entry.method, entry.path, stub);
        }

        let openapi_spec = generate_openapi_spec(
            "Rust WebX API",
            env!("CARGO_PKG_VERSION"),
            localizer.as_ref(),
        );
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
        let live_body =
            serde_json::to_vec(&serde_json::json!({"status":"pass"})).unwrap_or_default();
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
            if let Some(source) = spa_source.as_ref() {
                tracing::info!("    SPA Root: {}", source.disk_root());
                if let Some(assets) = source.embedded() {
                    tracing::info!(
                        "    Embedded: {} files, {:.1} MiB from {:.1} MiB (built from {})",
                        assets.len(),
                        assets.total_bytes() as f64 / (1024.0 * 1024.0),
                        assets.total_raw_bytes() as f64 / (1024.0 * 1024.0),
                        assets.root(),
                    );
                }
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
        let router_handler = make_router_handler(Arc::clone(&router), spa_fallback);

        // Resolve all registered hosted services from the DI container.
        // These will be started when `run()` is called and stopped on shutdown.
        let hosted_services: Vec<Arc<dyn IHostedService>> =
            provider.get_all::<dyn IHostedService>();

        Host {
            provider,
            dispatch_runtime,
            options,
            endpoints,
            localizer,
            body_limits,
            pipeline,
            router,
            router_handler,
            mode: self.mode,
            spa_source,
            shutdown: Arc::new(tokio::sync::watch::channel(false).0),
            hosted_services,
        }
    }
}

/// Handle for a running server, allowing programmatic graceful shutdown.
pub struct ServerHandle {
    shutdown: ShutdownSignal,
}

impl ServerHandle {
    /// Signal the server to stop accepting new connections and begin
    /// draining existing ones.
    ///
    /// The signal is stored in a `watch` channel, so calling this before the
    /// server's accept loops start is not lost.
    pub fn shutdown(self) {
        trigger_shutdown(&self.shutdown);
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

    /// Root DI container for this host instance.
    pub fn provider(&self) -> &Arc<ServiceProvider> {
        &self.provider
    }

    /// Instance-scoped dispatch runtime (provider + handler cache).
    pub fn dispatch_runtime(&self) -> &Arc<DispatchRuntime> {
        &self.dispatch_runtime
    }

    /// Endpoint metadata for this host.
    ///
    /// Built once from the compile-time route table; use it instead of reading
    /// `inventory` directly. Resolve names through [`Self::localizer`] or the
    /// DI-registered `IStringLocalizer`.
    pub fn endpoints(&self) -> &dyn IEndpointCatalog {
        &*self.endpoints
    }

    /// Localizer for this host's configured `Culture.Default`.
    pub fn localizer(&self) -> &dyn IStringLocalizer {
        &*self.localizer
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
                    return Err(webx_core::error::Error::Http(format!(
                        "Unsupported URL scheme '{}' in '{}'",
                        other, url
                    )))
                }
            }
        }

        let acceptor = if !https_addrs.is_empty() {
            let tls = &self.options.tls;
            if tls.cert_path.is_empty() || tls.key_path.is_empty() {
                return Err(webx_core::error::Error::Http(
                    "HTTPS URLs require Tls.CertPath and Tls.KeyPath".into(),
                ));
            }
            Some(build_tls_acceptor(&tls.cert_path, &tls.key_path)?)
        } else {
            None
        };

        // Bind every listener *before* starting hosted services or spawning
        // accept loops. A port conflict must fail `run()`, not be logged and
        // forgotten (which used to leave the process exiting with code 0).
        let mut http_listeners = Vec::with_capacity(http_addrs.len());
        for addr in &http_addrs {
            http_listeners.push(bind_listener(addr, "HTTP").await?);
        }
        let mut https_listeners = Vec::with_capacity(https_addrs.len());
        for addr in &https_addrs {
            https_listeners.push(bind_listener(addr, "HTTPS").await?);
        }

        start_hosted_services(&self.hosted_services, &self.dispatch_runtime).await?;

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

        install_shutdown_handler(Arc::clone(&self.shutdown));
        spawn_async_shutdown_listener(Arc::clone(&self.shutdown));

        let mut handles = Vec::new();
        let pipeline = Arc::clone(&self.pipeline);
        let router_handler = self.router_handler.clone();
        let dispatch_runtime = Arc::clone(&self.dispatch_runtime);
        let mode = self.mode;
        let body_limits = self.body_limits.clone();
        let max_connections = self.options.app.max_connections;

        for (addr, listener) in http_addrs.into_iter().zip(http_listeners) {
            handles.push(tokio::spawn(serve_http(
                addr,
                listener,
                self.shutdown.subscribe(),
                Arc::clone(&pipeline),
                router_handler.clone(),
                Arc::clone(&dispatch_runtime),
                mode,
                body_limits.clone(),
                max_connections,
            )));
        }

        if let Some(ref tls_acceptor) = acceptor {
            for (addr, listener) in https_addrs.into_iter().zip(https_listeners) {
                handles.push(tokio::spawn(serve_https(
                    addr,
                    tls_acceptor.clone(),
                    listener,
                    self.shutdown.subscribe(),
                    Arc::clone(&pipeline),
                    router_handler.clone(),
                    Arc::clone(&dispatch_runtime),
                    mode,
                    body_limits.clone(),
                    max_connections,
                )));
            }
        }

        // Wait for all listeners to finish (they exit after the shutdown
        // signal); a panicked listener is a host failure, not a silent exit.
        let mut run_error: Option<webx_core::error::Error> = None;
        for h in handles {
            if let Err(join_err) = h.await {
                if run_error.is_none() {
                    run_error = Some(webx_core::error::Error::Internal(format!(
                        "a listener task failed: {join_err}"
                    )));
                }
            }
        }

        stop_hosted_services(&self.hosted_services, &self.dispatch_runtime).await;

        match run_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Start the server at a single explicit address (convenience wrapper).
    pub async fn run_at(&self, addr: &str) -> Result<()> {
        let listener = bind_listener(addr, "HTTP").await?;
        start_hosted_services(&self.hosted_services, &self.dispatch_runtime).await?;

        install_shutdown_handler(Arc::clone(&self.shutdown));
        spawn_async_shutdown_listener(Arc::clone(&self.shutdown));
        serve_http(
            addr.to_string(),
            listener,
            self.shutdown.subscribe(),
            Arc::clone(&self.pipeline),
            self.router_handler.clone(),
            Arc::clone(&self.dispatch_runtime),
            self.mode,
            self.body_limits.clone(),
            self.options.app.max_connections,
        )
        .await;

        stop_hosted_services(&self.hosted_services, &self.dispatch_runtime).await;
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
        trigger_shutdown(&self.shutdown);
        Ok(())
    }
}

fn make_router_handler(router: Arc<Router>, spa_fallback: Option<Arc<SpaMiddleware>>) -> HandlerFn {
    Arc::new(move |ctx: &mut dyn IHttpContext| {
        let router = Arc::clone(&router);
        let spa_fallback = spa_fallback.clone();
        Box::pin(async move {
            // The context may carry a rejection detected from the request line
            // (oversized declared Content-Length). Failing here — inside the
            // pipeline — keeps CORS/security headers/logging/metrics in play.
            if let Some(err) = ctx.take_early_error() {
                return Err(err);
            }
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
                    let allowed = if path_exists {
                        router.allowed_methods(ctx.request().path())
                    } else {
                        Vec::new()
                    };
                    drop(router);
                    // Don't overwrite if a middleware (e.g. a static file
                    // response) already wrote a body.
                    if !ctx.response().has_body() {
                        // SPA routing: only a genuine router miss gets the app
                        // shell, and only when the client asked for HTML.
                        if let Some(spa) = spa_fallback {
                            if spa.try_index_fallback(ctx).await? {
                                return Ok(());
                            }
                        }
                        if path_exists {
                            if ctx.request().method().eq_ignore_ascii_case("OPTIONS") {
                                // Method discovery (RFC 9110 §9.3.7): CORS
                                // preflights are answered earlier, so this is a
                                // plain OPTIONS and must report what the path
                                // actually allows.
                                ctx.response_mut().set_status(204);
                                ctx.response_mut().set_header("allow", &allowed.join(", "));
                            } else {
                                // RFC 9110 §15.5.6: a 405 must carry `Allow`.
                                write_method_not_allowed(ctx, &allowed).await;
                            }
                        } else {
                            write_plain_error(ctx, 404, "Not Found").await;
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
    dispatch_runtime: Arc<DispatchRuntime>,
    body_limits: BodyLimits,
    remote_addr: Option<std::net::SocketAddr>,
) -> std::result::Result<hyper::Response<RespBody>, std::convert::Infallible> {
    let mut ctx = HttpContext::new(req, body_limits, remote_addr);

    let result = dispatch_runtime
        .run(async { pipeline.execute(&mut ctx, router_handler).await })
        .await;
    if let Err(e) = result {
        match e {
            // A binding/validation failure already carries its full
            // ProblemDetails (including the `errors` extension); write it
            // verbatim instead of flattening it into a detail string.
            webx_core::error::Error::Problem(problem) => {
                write_problem_response(&mut ctx, *problem).await
            }
            other => {
                write_framework_error(&mut ctx, &other).await;
            }
        }
    }

    Ok(ctx.into_response().await)
}

/// Write an error the framework itself produced (not an `Error::Problem`).
///
/// * 401 goes through the single unauthorized writer so every challenge is
///   identical and carries `WWW-Authenticate`; the variant's own message
///   ("authentication required" vs "invalid or expired token") is kept so a
///   client can tell missing credentials from a rejected token.
/// * 5xx never echoes the internal message: it is logged and replaced with a
///   stable detail, because database errors and file paths must not reach
///   clients.
/// * 4xx keep their (client-facing) detail, taken from the variant without the
///   operator-facing `Display` prefix.
async fn write_framework_error(ctx: &mut dyn IHttpContext, error: &webx_core::error::Error) {
    let status = error.status_code();
    if status == 401 {
        write_unauthorized(
            ctx,
            error.public_detail().unwrap_or("authentication required"),
        )
        .await;
    } else if status >= 500 {
        tracing::error!(status, error = %error, "unhandled server error");
        write_problem_response(ctx, build_problem(status, "an unexpected error occurred")).await;
    } else {
        write_problem_response(
            ctx,
            build_problem(status, error.public_detail().unwrap_or("request failed")),
        )
        .await;
    }
}

/// Write a framework-generated 4xx that has no [`Error`] value behind it
/// (404 from an unmatched route).
async fn write_plain_error(ctx: &mut dyn IHttpContext, status: u16, message: &str) {
    write_problem_response(ctx, build_problem(status, message)).await;
}

/// Write a 405 with the `Allow` header RFC 9110 §15.5.6 requires.
async fn write_method_not_allowed(ctx: &mut dyn IHttpContext, allowed: &[&str]) {
    let allow = allowed.join(", ");
    ctx.response_mut().set_header("allow", &allow);
    write_plain_error(ctx, 405, "Method Not Allowed").await;
}

// ---------------------------------------------------------------------------
// Shutdown helpers
// ---------------------------------------------------------------------------

/// Raise the shutdown flag.
///
/// `watch::Sender::send` stores the value, so a listener that subscribes later
/// (or a listener between two `select!` iterations) still observes it — unlike
/// `Notify::notify_waiters`, which drops the wakeup when nobody is parked.
fn trigger_shutdown(shutdown: &ShutdownSignal) {
    // `send_replace` stores the value unconditionally. `send` would fail while
    // no receiver exists (e.g. `handle.shutdown()` called before `run()`
    // subscribes) and silently drop the signal — the exact bug this type
    // replaces `Notify` for.
    shutdown.send_replace(true);
}

/// Resolve once the host has been asked to shut down.
///
/// Returns immediately when the flag is already set; also returns when the
/// sender disappears (host dropped) so a connection task never hangs forever.
async fn wait_for_shutdown(shutdown: &mut tokio::sync::watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    loop {
        if shutdown.changed().await.is_err() {
            return;
        }
        if *shutdown.borrow() {
            return;
        }
    }
}

/// Register a process-wide Ctrl+C / SIGINT handler.
///
/// On Windows, `cargo run` may exit the parent without stopping the child;
/// registering here ensures the server process itself receives the signal.
/// A second Ctrl+C forces immediate exit.
///
/// `ctrlc::set_handler` can only be installed once per process, so every host
/// registers its signal (weakly) here and the single handler raises all of
/// them. That keeps two hosts in one process — integration tests, embedded
/// scenarios — individually stoppable by Ctrl+C.
fn install_shutdown_handler(shutdown: ShutdownSignal) {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock, Weak};

    static HANDLERS: OnceLock<Mutex<Vec<Weak<tokio::sync::watch::Sender<bool>>>>> = OnceLock::new();
    static CTRL_COUNT: AtomicUsize = AtomicUsize::new(0);
    static INSTALLED: AtomicBool = AtomicBool::new(false);

    let handlers = HANDLERS.get_or_init(|| Mutex::new(Vec::new()));
    {
        let mut registered = handlers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Drop hosts that have already exited before adding this one.
        registered.retain(|weak| weak.strong_count() > 0);
        registered.push(Arc::downgrade(&shutdown));
    }

    // `ctrlc::set_handler` is process-wide and may only be called once. The
    // flag (not the registry length) decides: hosts may come and go, but the
    // handler stays installed and raises whichever hosts are alive.
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }

    if let Err(e) = ctrlc::set_handler(move || {
        let n = CTRL_COUNT.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            tracing::info!("Shutdown signal received, draining connections...");
            let registered = handlers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for weak in registered.iter() {
                if let Some(shutdown) = weak.upgrade() {
                    trigger_shutdown(&shutdown);
                }
            }
        } else {
            tracing::warn!("Second interrupt — force exiting.");
            std::process::exit(130);
        }
    }) {
        tracing::warn!("Could not install Ctrl+C handler: {}", e);
    }
}

/// Tokio-based listener for Ctrl+C (all platforms) and SIGTERM (Unix).
fn spawn_async_shutdown_listener(shutdown: ShutdownSignal) {
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        tracing::info!("Async shutdown signal received, draining connections...");
        trigger_shutdown(&shutdown);
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

/// How long a hosted service may take to start before the host gives up.
const HOSTED_START_TIMEOUT_SECS: u64 = 30;
/// How long a hosted service may take to stop before it is skipped.
const HOSTED_STOP_TIMEOUT_SECS: u64 = 30;

async fn start_hosted_services(
    services: &[Arc<dyn IHostedService>],
    dispatch_runtime: &Arc<DispatchRuntime>,
) -> Result<()> {
    if services.is_empty() {
        return Ok(());
    }
    tracing::info!("Starting {} hosted service(s)...", services.len());
    let mut started = 0usize;
    let mut failure: Option<webx_core::error::Error> = None;

    for svc in services {
        // Each start gets its own DI scope: a `dispatch_provider().get_owned()`
        // inside `start()` resolves scoped dependencies (DbContext, connections)
        // against this scope instead of the root provider's cache, so they are
        // disposed when the start finishes rather than living for the process.
        let attempt = tokio::time::timeout(
            std::time::Duration::from_secs(HOSTED_START_TIMEOUT_SECS),
            dispatch_runtime.run_in_scope(async { svc.start().await }),
        )
        .await;
        match attempt {
            Ok(Ok(())) => started += 1,
            Ok(Err(err)) => {
                failure = Some(err);
                break;
            }
            Err(_) => {
                failure = Some(webx_core::error::Error::Internal(format!(
                    "hosted service start timed out after {HOSTED_START_TIMEOUT_SECS}s"
                )));
                break;
            }
        }
    }

    match failure {
        None => {
            tracing::info!("All {started} hosted service(s) started.");
            Ok(())
        }
        Some(err) => {
            // Roll back the services that did start, newest first, so a failed
            // startup does not leave pools/loops running.
            for (index, svc) in services[..started].iter().enumerate().rev() {
                stop_hosted_service(dispatch_runtime, index, svc).await;
            }
            Err(err)
        }
    }
}

async fn stop_hosted_services(
    services: &[Arc<dyn IHostedService>],
    dispatch_runtime: &Arc<DispatchRuntime>,
) {
    if services.is_empty() {
        return;
    }
    tracing::info!("Stopping {} hosted service(s)...", services.len());
    for (index, svc) in services.iter().enumerate().rev() {
        stop_hosted_service(dispatch_runtime, index, svc).await;
    }
    tracing::info!("All hosted services stopped.");
}

/// Stop one service with a timeout (and a fresh DI scope), logging failures.
async fn stop_hosted_service(
    dispatch_runtime: &Arc<DispatchRuntime>,
    index: usize,
    svc: &Arc<dyn IHostedService>,
) {
    match tokio::time::timeout(
        std::time::Duration::from_secs(HOSTED_STOP_TIMEOUT_SECS),
        dispatch_runtime.run_in_scope(async { svc.stop().await }),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            tracing::warn!(service_index = index, error = %err, "hosted service stop error")
        }
        Err(_) => tracing::error!(
            service_index = index,
            timeout_secs = HOSTED_STOP_TIMEOUT_SECS,
            "hosted service stop timed out"
        ),
    }
}

async fn drain_connections(join_set: &mut JoinSet<()>, label: &str, mode: AppMode) {
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

/// Bind a TCP listener, turning an OS-level failure into a host error.
///
/// Binding happens before any accept loop starts, so a port conflict fails
/// `run()`/`run_at()` with a non-zero exit instead of a log line and a silent
/// `Ok(())`.
async fn bind_listener(addr: &str, label: &str) -> Result<tokio::net::TcpListener> {
    tokio::net::TcpListener::bind(addr).await.map_err(|err| {
        webx_core::error::Error::Http(format!("failed to bind {label} on {addr}: {err}"))
    })
}

/// Parse a URL string into (scheme, addr) pair.
/// e.g., "https://0.0.0.0:5030" →("https", "0.0.0.0:5030")
fn parse_url(url: &str) -> Result<(&str, String)> {
    if let Some(rest) = url.strip_prefix("https://") {
        Ok(("https", rest.to_string()))
    } else if let Some(rest) = url.strip_prefix("http://") {
        Ok(("http", rest.to_string()))
    } else {
        Err(webx_core::error::Error::Http(format!(
            "Invalid URL '{}'. Use http://host:port or https://host:port",
            url
        )))
    }
}

/// Serve plain HTTP on an already-bound listener.
#[allow(clippy::too_many_arguments)]
async fn serve_http(
    addr: String,
    listener: tokio::net::TcpListener,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    pipeline: Arc<MiddlewarePipeline>,
    router_handler: HandlerFn,
    dispatch_runtime: Arc<DispatchRuntime>,
    mode: AppMode,
    body_limits: BodyLimits,
    max_connections: usize,
) {
    let sem = Arc::new(tokio::sync::Semaphore::new(max_connections));
    let mut join_set = JoinSet::new();

    loop {
        // Check the stored flag first: a shutdown requested before this loop
        // started (the documented `spawn(run); handle.shutdown()` pattern) must
        // not be missed.
        if *shutdown.borrow() {
            tracing::info!("HTTP {}: stop accepting, draining...", addr);
            break;
        }
        tokio::select! {
            _ = shutdown.changed() => {
                tracing::info!("HTTP {}: stop accepting, draining...", addr);
                break;
            }
            accept_result = listener.accept() => {
                let (stream, peer_addr) = match accept_result {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!("Accept error (will retry): {}", e);
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let remote_addr = Some(peer_addr);

                while join_set.try_join_next().is_some() {}

                let permit = match sem.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                let io = TokioIo::new(stream);
                let pipeline = Arc::clone(&pipeline);
                let router_handler = router_handler.clone();
                let dispatch_runtime = Arc::clone(&dispatch_runtime);
                let body_limits = body_limits.clone();
                let conn_shutdown = shutdown.clone();

                join_set.spawn(async move {
                    let _permit = permit;
                    let svc_fn = service_fn(move |req: Request<Incoming>| {
                        let pipeline = Arc::clone(&pipeline);
                        let router_handler = router_handler.clone();
                        let dispatch_runtime = Arc::clone(&dispatch_runtime);
                        let body_limits = body_limits.clone();
                        let mode = mode;
                        // Peer address for rate limiting and audit; captured per
                        // connection and carried into the request context.
                        let remote_addr = remote_addr;
                        async move {
                            let start = Instant::now();
                            let method = req.method().to_string();
                            let path = req.uri().path().to_string();
                            let result = handle_request(
                                req,
                                pipeline,
                                router_handler,
                                dispatch_runtime,
                                body_limits,
                                remote_addr,
                            )
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

                    let conn = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, svc_fn);
                    let mut conn = std::pin::pin!(conn);
                    let mut conn_shutdown = conn_shutdown;
                    tokio::select! {
                        result = conn.as_mut() => {
                            if let Err(err) = result {
                                tracing::error!("Connection error: {}", err);
                            }
                        }
                        _ = wait_for_shutdown(&mut conn_shutdown) => {
                            // Let in-flight requests finish and answer with
                            // `Connection: close` instead of aborting the socket
                            // after the drain timeout.
                            conn.as_mut().graceful_shutdown();
                            if let Err(err) = conn.await {
                                tracing::debug!(error = %err, "connection closed during shutdown");
                            }
                        }
                    }
                });
            }
        }
    }

    drain_connections(&mut join_set, &format!("HTTP {}", addr), mode).await;
}

/// Serve HTTPS (TLS) on an already-bound listener.
#[allow(clippy::too_many_arguments)]
async fn serve_https(
    addr: String,
    acceptor: TlsAcceptor,
    listener: tokio::net::TcpListener,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    pipeline: Arc<MiddlewarePipeline>,
    router_handler: HandlerFn,
    dispatch_runtime: Arc<DispatchRuntime>,
    mode: AppMode,
    body_limits: BodyLimits,
    max_connections: usize,
) {
    let sem = Arc::new(tokio::sync::Semaphore::new(max_connections));
    let mut join_set = JoinSet::new();

    loop {
        if *shutdown.borrow() {
            tracing::info!("HTTPS {}: stop accepting, draining...", addr);
            break;
        }
        tokio::select! {
            _ = shutdown.changed() => {
                tracing::info!("HTTPS {}: stop accepting, draining...", addr);
                break;
            }
            accept_result = listener.accept() => {
                let (stream, peer_addr) = match accept_result {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!("Accept error (will retry): {}", e);
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let remote_addr = Some(peer_addr);

                while join_set.try_join_next().is_some() {}

                let permit = match sem.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                let acceptor = acceptor.clone();
                let pipeline = Arc::clone(&pipeline);
                let router_handler = router_handler.clone();
                let dispatch_runtime = Arc::clone(&dispatch_runtime);
                let body_limits = body_limits.clone();
                let conn_shutdown = shutdown.clone();

                join_set.spawn(async move {
                    let _permit = permit;
                    match acceptor.accept(stream).await {
                        Ok(tls_stream) => {
                            let io = TokioIo::new(tls_stream);
                            let svc_fn = service_fn(move |req: Request<Incoming>| {
                                let pipeline = Arc::clone(&pipeline);
                                let router_handler = router_handler.clone();
                                let dispatch_runtime = Arc::clone(&dispatch_runtime);
                                let body_limits = body_limits.clone();
                                let mode = mode;
                                // Peer address for rate limiting and audit.
                                let remote_addr = remote_addr;
                                async move {
                                    let start = Instant::now();
                                    let method = req.method().to_string();
                                    let path = req.uri().path().to_string();
                                    let result = handle_request(
                                        req,
                                        pipeline,
                                        router_handler,
                                        dispatch_runtime,
                                        body_limits,
                                        remote_addr,
                                    )
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

                            let conn = hyper::server::conn::http1::Builder::new()
                                .serve_connection(io, svc_fn);
                            let mut conn = std::pin::pin!(conn);
                            let mut conn_shutdown = conn_shutdown;
                            tokio::select! {
                                result = conn.as_mut() => {
                                    if let Err(err) = result {
                                        tracing::error!("TLS connection error: {}", err);
                                    }
                                }
                                _ = wait_for_shutdown(&mut conn_shutdown) => {
                                    conn.as_mut().graceful_shutdown();
                                    if let Err(err) = conn.await {
                                        tracing::debug!(error = %err, "TLS connection closed during shutdown");
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("TLS handshake error: {}", e);
                        }
                    }
                });
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
        return Err(webx_core::error::Error::Http(
            "TLS certificate or key path not configured.".into(),
        ));
    }

    let cert_file = File::open(cert_path).map_err(|e| {
        webx_core::error::Error::Http(format!("Cannot open cert '{}': {}", cert_path, e))
    })?;
    let mut cert_reader = BufReader::new(cert_file);
    let certs: Vec<CertificateDer> = certs(&mut cert_reader).filter_map(|r| r.ok()).collect();
    if certs.is_empty() {
        return Err(webx_core::error::Error::Http(format!(
            "No valid certs in '{}'",
            cert_path
        )));
    }

    let key_file = File::open(key_path).map_err(|e| {
        webx_core::error::Error::Http(format!("Cannot open key '{}': {}", key_path, e))
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
            webx_core::error::Error::Http(format!("No valid private key in '{}'", key_path))
        })?;

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| webx_core::error::Error::Http(format!("TLS config error: {}", e)))?;

    Ok(TlsAcceptor::from(std::sync::Arc::new(config)))
}

#[cfg(test)]
mod embedded_attach_tests {
    use super::with_embedded_assets;
    use webx_spa::{ContentEncoding, EmbeddedAsset, EmbeddedAssets, SpaSource};

    const FILES: &[EmbeddedAsset] = &[EmbeddedAsset {
        path: "index.html",
        bytes: b"hi",
        raw_len: 2,
        encoding: ContentEncoding::Identity,
        content_type: "text/html",
        etag: "\"x\"",
    }];

    #[test]
    fn no_table_leaves_disk_source() {
        let source = Some(SpaSource::Disk("wwwroot".into()));
        let merged = with_embedded_assets(source, None);
        assert!(matches!(merged, Some(SpaSource::Disk(ref r)) if r == "wwwroot"));
    }

    #[test]
    fn table_layers_under_use_spa_overlay() {
        let assets = EmbeddedAssets::new("../wwwroot", FILES);
        let merged = with_embedded_assets(Some(SpaSource::Disk("wwwroot".into())), Some(assets));
        match merged {
            Some(SpaSource::Embedded {
                overlay_root,
                assets,
            }) => {
                assert_eq!(overlay_root, "wwwroot");
                assert_eq!(assets.root(), "../wwwroot");
            }
            other => panic!("expected embedded, got {other:?}"),
        }
    }

    #[test]
    fn table_alone_uses_build_root_as_overlay() {
        let assets = EmbeddedAssets::new("../wwwroot", FILES);
        let merged = with_embedded_assets(None, Some(assets));
        match merged {
            Some(SpaSource::Embedded { overlay_root, .. }) => {
                assert_eq!(overlay_root, "../wwwroot");
            }
            other => panic!("expected embedded, got {other:?}"),
        }
    }

    #[test]
    fn host_crate_registers_no_asset_table() {
        assert!(EmbeddedAssets::registered().is_none());
    }
}
