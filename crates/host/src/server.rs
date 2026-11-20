//! Host builder and hyper server integration.
//!
//! Includes built-in exception middleware: errors produced by endpoints
//! are caught and converted to well-formed HTTP error responses using
//! `Error::status_code()`.

use rust_dix::{ServiceCollection, ServiceProvider};
use webx_core::app::IHost;
use webx_core::auth::IAuthenticationScheme;
use webx_core::config::{self, AppOptions};
use webx_core::error::{Error, IErrorResponseWriter, Result};
use webx_core::handler::IHostedService;
use webx_core::hosted::{CancellationToken, ShutdownToken};
use webx_core::localization::{IStringLocalizer, LocalizationOptions, StringLocalizer};
use webx_core::middleware::IMiddleware;
use webx_core::mode::AppMode;
use webx_core::route::catalog::{EndpointCatalog, IEndpointCatalog};
use webx_core::routing::{HttpMethod, IEndpoint};
use webx_core::security::{ISessionStore, ITokenIssuer};

use crate::auth::{AuthenticationMiddleware, JwtScheme};
use crate::auth_jwt::{init_jwt_secret, JwtAuth, JwtSecret, JwtTokenIssuer};
use crate::cors::{CorsConfig, CorsMiddleware};
use crate::endpoint::{StaticHtmlEndpoint, StaticJsonEndpoint, StubEndpoint};
use crate::health::{
    HealthCheckEndpoint, HealthCheckRegistry, HealthStatus, IHealthCheck,
    DEFAULT_HEALTH_CHECK_TIMEOUT,
};
use crate::memory_cache::MemoryCache;
use crate::metrics::{HttpMetrics, MetricsEndpoint, MetricsMiddleware};
use crate::pipeline::MiddlewarePipeline;
use crate::rate_limit::RateLimitMiddleware;
use jsonwebtoken::{DecodingKey, Validation};
use webx_core::route::scan::RouteEntry;
use webx_core::DispatchRuntime;
use webx_openapi::{generate_openapi_spec, APIUI_HTML};
use webx_spa::{EmbeddedAssets, SpaMiddleware, SpaSource};

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

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

/// Bound on `IHostedService::start` during host startup.
const HOSTED_START_TIMEOUT_SECS: u64 = 30;
/// Bound on `IHostedService::stop` during graceful shutdown.
const HOSTED_STOP_TIMEOUT_SECS: u64 = 30;

pub struct Host {
    #[allow(dead_code)]
    provider: Arc<ServiceProvider>,
    dispatch_runtime: Arc<DispatchRuntime>,
    pub options: AppOptions,
    /// Endpoint metadata for this host, built once from the compile-time routes.
    endpoints: Arc<dyn IEndpointCatalog>,
    /// Resolves endpoint resource keys for the host's configured culture.
    localizer: Arc<dyn IStringLocalizer>,
    /// 2.0 execution engine state (workers, router, pipeline).
    engine: Arc<webx_engine::Shared>,
    /// Kept for introspection; the middleware already holds its own copy.
    #[allow(dead_code)]
    spa_source: Option<SpaSource>,
    shutdown: ShutdownSignal,
    /// Hosted services that are started on host start
    /// and stopped on graceful shutdown.
    hosted_services: Arc<Vec<Arc<dyn IHostedService>>>,
    /// Set once the hosted services were stopped: shutdown stops them as soon as
    /// the signal arrives (before connections drain), and the teardown path calls
    /// in again; both go through this cell so `stop()` runs exactly once.
    hosted_stopped: Arc<tokio::sync::OnceCell<()>>,
    /// Cancelled when the host starts graceful shutdown; registered in DI as
    /// [`ShutdownToken`] so long-lived SSE responses can end cleanly.
    shutdown_token: CancellationToken,
    /// Single-use lifecycle state; see [`HostState`].
    state: std::sync::atomic::AtomicU8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HostState {
    Built,
    Running,
    Disposed,
    Failed,
}

impl HostState {
    fn load(self) -> u8 {
        self as u8
    }
}

/// Cancellation guard for one host run.
struct HostRunGuard<'a> {
    host: &'a Host,
    armed: bool,
}

impl<'a> HostRunGuard<'a> {
    fn new(host: &'a Host) -> Self {
        Self { host, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for HostRunGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.host
                .state
                .store(HostState::Failed.load(), Ordering::Release);
            self.host.request_immediate_shutdown();
        }
    }
}

/// Factory for a user-registered middleware.
type MiddlewareFactory = Box<dyn Fn() -> Arc<dyn IMiddleware> + Send + Sync>;

/// A deferred service registration.
///
/// Receives the fully merged [`AppOptions`] — including a resolved
/// `Jwt.SecretFile` — so a registration that constructs a security primitive
/// reads the same configuration the host itself uses.
type ServiceConfig = Box<dyn FnOnce(ServiceCollection, &AppOptions) -> ServiceCollection + Send>;

/// A user middleware waiting to be placed in the pipeline at its slot.
struct MiddlewareSpec {
    slot: webx_core::middleware::MiddlewareSlot,
    factory: MiddlewareFactory,
}

#[allow(clippy::type_complexity)]
pub struct HostBuilder {
    service_configs: Vec<ServiceConfig>,
    mode: AppMode,
    spa_source: Option<SpaSource>,
    spa_disabled: bool,
    options_modifiers: Vec<Box<dyn FnOnce(&mut AppOptions) + Send>>,
    cors_config: Option<CorsConfig>,
    auth_enabled: bool,
    /// Additional authentication schemes, tried before the JWT scheme.
    auth_schemes: Vec<Arc<dyn IAuthenticationScheme>>,
    localization: Option<LocalizationOptions>,
    health_registry: Arc<HealthCheckRegistry>,
    /// Middlewares registered through the builder, kept with their slot so
    /// ordering does not depend on DI enumeration order.
    middleware_specs: Vec<MiddlewareSpec>,
    /// Error writers registered through the builder, tried in order.
    error_writers: Vec<Arc<dyn IErrorResponseWriter>>,
    /// Named outbound clients registered through `add_http_client`.
    http_clients: Vec<(String, Arc<dyn webx_client::IHttpClient>)>,
    /// Opt back into the pre-1.0 lenient root-scope behavior (rust-dix 0.7).
    /// Migration-only: 1.0 defaults to strict scope validation.
    lenient_scope_validation: bool,
}

/// Everything `prepare` collects before the DI provider is built.
///
/// Splitting at the provider boundary is what lets `try_build` and
/// `try_build_async` share the whole assembly (options, host defaults, pipeline,
/// router) while choosing rust-dix's sync or async provider builder.
struct PreparedHost {
    builder: HostBuilder,
    svc: ServiceCollection,
    provider_options: rust_dix::ServiceProviderOptions,
    options: AppOptions,
    endpoints: Arc<dyn IEndpointCatalog>,
    localizer: Arc<dyn IStringLocalizer>,
    handler_cache: Arc<webx_core::route::scan::HandlerCache>,
    /// Host shutdown token, created before the DI provider so it can be
    /// registered as a service instance.
    shutdown_token: CancellationToken,
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

    /// Snake-case alias for [`HostAppBuilder::useOptions`].
    pub fn use_options<F>(&mut self, f: F)
    where
        F: FnOnce(&mut AppOptions) + Send + 'static,
    {
        self.useOptions(f);
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
            auth_schemes: Vec::new(),
            localization: None,
            health_registry: HealthCheckRegistry::new(),
            middleware_specs: Vec::new(),
            error_writers: Vec::new(),
            http_clients: Vec::new(),
            lenient_scope_validation: false,
        }
    }

    /// Defer service registration until host build.
    ///
    /// The closure runs during [`Self::try_build`] / [`Self::try_build_async`],
    /// after configuration has been merged and `Jwt.SecretFile` resolved, and
    /// before the host's own defaults are registered. Use
    /// [`Self::register_with_options`] when the registration itself needs the
    /// final [`AppOptions`].
    pub fn register<F>(self, f: F) -> Self
    where
        F: FnOnce(ServiceCollection) -> ServiceCollection + Send + 'static,
    {
        self.register_with_options(move |svc, _options| f(svc))
    }

    /// Like [`Self::register`], but the closure also receives the final
    /// [`AppOptions`].
    ///
    /// `options` is the appsettings / environment / `useOptions` merge **after**
    /// `Jwt.SecretFile` was resolved into `Jwt.Secret`. A custom
    /// [`ITokenIssuer`] can therefore be
    /// built from exactly the key the host's JWT middleware verifies with — no
    /// second read of the secret file, no empty-key signature in a
    /// `SecretFile`-only deployment:
    ///
    /// ```ignore
    /// use std::time::Duration;
    ///
    /// Host::builder()
    ///     .add_authentication()
    ///     .register_with_options(|svc, options| {
    ///         let issuer = JwtTokenIssuer::from_secret(
    ///             options.jwt.secret.as_bytes(),
    ///             Duration::from_secs(3600),
    ///         );
    ///         svc.try_add_singleton::<dyn ITokenIssuer>(move |_| Arc::new(issuer))
    ///     })
    ///     .build()
    /// ```
    ///
    /// If the issuer is created lazily by a DI factory instead, resolve
    /// [`JwtSecret`]: the host registers it before
    /// these closures, and a missing value means no secret was configured
    /// (resolving it fails the provider build rather than signing with `""`).
    pub fn register_with_options<F>(mut self, f: F) -> Self
    where
        F: FnOnce(ServiceCollection, &AppOptions) -> ServiceCollection + Send + 'static,
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

    /// Keep the pre-1.0 (rust-dix 0.7) lenient root-scope behavior.
    ///
    /// 1.0 defaults to **strict** scope validation (MEDI `ValidateScopes = true`):
    /// resolving a Scoped service from the root `ServiceProvider` returns
    /// [`webx::Error::Di`](webx_core::error::Error::Di) instead of silently caching it on the
    /// root. Existing code that resolves Scoped services from
    /// `host.provider()` can opt back in here while it migrates to explicit
    /// scopes (`provider.scope()` / `get_owned`) — see
    /// `docs/rust-webx/16-migration/upgrade-to-1.0.md`.
    pub fn lenient_scope_validation(mut self) -> Self {
        self.lenient_scope_validation = true;
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

    /// Bind a configuration section once and expose it through a hot-reloadable
    /// [`OptionsMonitor`](webx_core::options_monitor::OptionsMonitor).
    ///
    /// The monitor is registered twice, for both injection styles:
    ///
    /// * `Arc<OptionsMonitor<T>>` — concrete, with
    ///   [`OptionsMonitor::set`](webx_core::options_monitor::OptionsMonitor::set);
    /// * `Arc<dyn IOptionsMonitor<T>>` — the port, for handlers/services.
    ///
    /// Pair it with [`HostBuilder::watch_options_file`] (or a custom
    /// `PeriodicBackgroundService`) to publish new snapshots without a restart.
    ///
    /// ```ignore
    /// Host::builder()
    ///     .add_options_monitor::<GatewayConfig>("Gateway")
    ///     .build();
    /// ```
    pub fn add_options_monitor<T>(self, section: &str) -> Self
    where
        T: for<'de> serde::Deserialize<'de> + Default + Send + Sync + 'static,
    {
        let mode = self.mode;
        let section = section.to_string();
        self.register(move |svc| {
            let appsettings = config::load_appsettings(mode).unwrap_or_default();
            let bound: T = config::bind_config(&appsettings, &section);
            tracing::info!(
                "[Host] add_options_monitor<{}>(\"{}\") registered",
                std::any::type_name::<T>(),
                section
            );
            let monitor = Arc::new(webx_core::options_monitor::OptionsMonitor::new(bound));
            let port: Arc<dyn webx_core::options_monitor::IOptionsMonitor<T>> = monitor.clone();
            svc.instance(monitor).instance(port)
        })
    }

    /// Bind `section` at startup and poll `path` for replacements.
    ///
    /// The file is read on the worker interval; unchanged bytes are ignored and
    /// a malformed file leaves the current snapshot active (see
    /// [`OptionsFileWatcher`](crate::options_watch::OptionsFileWatcher)).
    /// The monitor is registered like [`HostBuilder::add_options_monitor`].
    ///
    /// ```ignore
    /// Host::builder()
    ///     .watch_options_file::<GatewayConfig>(
    ///         "Gateway",
    ///         "config/gateway.json",
    ///         Duration::from_secs(5),
    ///     )
    ///     .build();
    /// ```
    pub fn watch_options_file<T>(
        self,
        section: &str,
        path: impl Into<std::path::PathBuf>,
        interval: Duration,
    ) -> Self
    where
        T: for<'de> serde::Deserialize<'de> + Default + Send + Sync + 'static,
    {
        use webx_core::route::ext::IServiceCollectionExt;

        let mode = self.mode;
        let section = section.to_string();
        let path = path.into();
        self.register(move |svc| {
            let appsettings = config::load_appsettings(mode).unwrap_or_default();
            let bound: T = config::bind_config(&appsettings, &section);
            tracing::info!(
                "[Host] watch_options_file<{}>(\"{}\", {}) registered",
                std::any::type_name::<T>(),
                section,
                path.display()
            );
            let monitor = Arc::new(webx_core::options_monitor::OptionsMonitor::new(bound));
            let port: Arc<dyn webx_core::options_monitor::IOptionsMonitor<T>> = monitor.clone();
            let watcher = crate::options_watch::OptionsFileWatcher::new(
                Arc::clone(&monitor),
                section,
                path,
                interval,
            );
            svc.instance(monitor)
                .instance(port)
                .add_background_service_with(move |_| Arc::new(watcher.clone()))
        })
    }

    /// Register a named outbound HTTP client for injection.
    ///
    /// The host collects all `add_http_client` calls into one
    /// [`HttpClientRegistry`](webx_client::HttpClientRegistry) and registers it
    /// twice: as `Arc<HttpClientRegistry>` and as
    /// `Arc<dyn IHttpClientFactory>`. Handlers inject either form and resolve a
    /// provider by name (`clients.create("openai")?`) — no hyper code in the
    /// application.
    ///
    /// ```ignore
    /// Host::builder()
    ///     .add_http_client("openai", HttpClient::new("openai", options)?)
    ///     .build();
    /// ```
    ///
    /// A registry registered manually with `.register(...)` wins: the host
    /// default only fills the port when it is still empty.
    pub fn add_http_client(
        mut self,
        name: impl Into<String>,
        client: webx_client::HttpClient,
    ) -> Self {
        self.http_clients.push((
            name.into(),
            Arc::new(client) as Arc<dyn webx_client::IHttpClient>,
        ));
        self
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
    ///
    /// # Custom token issuers
    ///
    /// The host resolves `Jwt.SecretFile` before application service
    /// registrations and registers the result as
    /// [`JwtSecret`]. A custom
    /// [`ITokenIssuer`] therefore reaches the
    /// final key either through [`Self::register_with_options`] (read
    /// `options.jwt.secret` in the closure) or by resolving `JwtSecret` in a
    /// factory. The host's default issuer is added with `try_add`, so an issuer
    /// registered by the application wins.
    pub fn add_authentication(mut self) -> Self {
        self.auth_enabled = true;
        self
    }

    /// Register a custom authentication scheme.
    ///
    /// Schemes are tried in registration order, before the JWT scheme enabled
    /// by [`add_authentication`](Self::add_authentication); the first scheme
    /// whose `matches` returns `true` handles the request. This is how a
    /// management-plane JWT and a data-plane `sk-` API key coexist:
    ///
    /// ```ignore
    /// Host::builder()
    ///     .add_authentication_scheme(Arc::new(ApiKeyScheme::new(validator)))
    ///     .add_authentication() // JWT, tried after the API-key scheme
    ///     .build()
    /// ```
    ///
    /// A scheme that never matches leaves the request anonymous; a scheme that
    /// matches an explicitly presented but invalid credential returns 401.
    pub fn add_authentication_scheme(mut self, scheme: Arc<dyn IAuthenticationScheme>) -> Self {
        self.auth_schemes.push(scheme);
        self
    }

    /// Register an error-response writer.
    ///
    /// Writers are tried in registration order before the default RFC 7807
    /// `problem+json` path. This is how an OpenAI-compatible `/v1/` surface can
    /// answer with `{"error": {...}}` while the rest of the host keeps
    /// `problem+json`:
    ///
    /// ```ignore
    /// Host::builder()
    ///     .add_error_writer(Arc::new(OpenAiErrorWriter::new()))
    ///     .build();
    /// ```
    pub fn add_error_writer(mut self, writer: Arc<dyn IErrorResponseWriter>) -> Self {
        self.error_writers.push(writer);
        self
    }

    /// Register a session store so authentication rejects revoked tokens.
    ///
    /// After verifying a token, the authentication middleware asks the store
    /// whether the token's `jti` has been revoked; a revoked token is answered
    /// with the framework's 401 (problem+json + `WWW-Authenticate`) before the
    /// endpoint runs. Without a store, authentication only verifies signatures
    /// — the 1.1 behaviour.
    ///
    /// [`InMemorySessionStore`](webx_core::security::InMemorySessionStore) is
    /// the dependency-free store for a single-process deployment; pass a
    /// persistent [`ISessionStore`] implementation when revocations must survive
    /// a restart.
    ///
    /// **This is the only recommended entry point.** Revocation is enabled when
    /// the container has a `dyn ISessionStore` registration — this method
    /// registers exactly that. Registering the *concrete* type instead (for
    /// example `svc.instance(Arc::new(InMemorySessionStore::new()))`) leaves the
    /// `dyn` key unresolved and revocation silently stays off; register with
    /// `svc.try_add_singleton::<dyn ISessionStore>(...)` if not using this
    /// builder. The host logs the on/off decision at startup (debug level).
    ///
    /// `jwt_middleware_with_revocation` is the escape hatch for a hand-assembled
    /// middleware pipeline only; regular hosts use this method.
    ///
    /// ```ignore
    /// Host::builder()
    ///     .add_authentication()
    ///     .use_session_store(Arc::new(InMemorySessionStore::new()))
    ///     .build()
    /// ```
    pub fn use_session_store(mut self, store: Arc<dyn ISessionStore>) -> Self {
        self.service_configs.push(Box::new(move |svc, _options| {
            let store = Arc::clone(&store);
            svc.try_add_singleton::<dyn ISessionStore>(move |_| Arc::clone(&store))
        }));
        self
    }

    /// Replace the default password hasher registered in DI.
    ///
    /// Host startup registers `Argon2PasswordHasher` (feature
    /// `password-argon2`, on by default) so handlers can inject
    /// `Arc<dyn IPasswordHasher>` without any hashing code of their own. Use this
    /// to change the Argon2 cost parameters, or to plug a different algorithm;
    /// register through DI (`svc.singleton::<dyn IPasswordHasher>`) for the same
    /// effect when building services by hand.
    ///
    /// ```ignore
    /// // raise the Argon2 cost for this deployment
    /// Host::builder().use_password_hasher(Arc::new(
    ///     Argon2PasswordHasher::with_params(Argon2Params::new(65536, 3, 1)),
    /// ))
    /// ```
    pub fn use_password_hasher(
        mut self,
        hasher: Arc<dyn webx_core::security::IPasswordHasher>,
    ) -> Self {
        self.service_configs.push(Box::new(move |svc, _options| {
            let hasher = Arc::clone(&hasher);
            svc.try_add_singleton::<dyn webx_core::security::IPasswordHasher>(move |_| {
                Arc::clone(&hasher)
            })
        }));
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
            .push(Box::new(move |svc, _options| svc.instance(cache)));
        self
    }

    /// Register a memory cache with custom configuration.
    pub fn add_memory_cache_with(mut self, max_entries: usize) -> Self {
        let cache = Arc::new(MemoryCache::new().with_max_entries(max_entries));
        self.service_configs.push(Box::new(move |svc, _options| {
            svc.instance(Arc::clone(&cache))
        }));
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

    /// Register a **synchronous** health check probe.
    ///
    /// Health checks are exposed via `/health` and `/health/ready` endpoints
    /// following RFC 8407 (`application/health+json` content type).
    ///
    /// The closure runs on the request task and cannot be timed out: use it only
    /// for cheap, non-blocking checks. A check that must `.await` I/O (database
    /// ping, HTTP dependency, queue depth) belongs in
    /// [`Self::add_health_check_async`]; the sync view then reports it as `warn`
    /// instead of evaluating it.
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

    /// Register an **async** health check with the default timeout
    /// ([`DEFAULT_HEALTH_CHECK_TIMEOUT`], 3 s).
    ///
    /// Async checks are evaluated concurrently on every `/health` /
    /// `/health/ready` request, each under its own timeout. A check that exceeds
    /// the timeout is reported as `fail` (`timed out after ...`) and readiness
    /// answers 503; `warn` keeps the endpoint at 200.
    ///
    /// ```ignore
    /// struct DatabaseProbe { db: Arc<Pool> }
    ///
    /// #[async_trait::async_trait]
    /// impl IHealthCheck for DatabaseProbe {
    ///     fn name(&self) -> &str { "database" }
    ///     async fn check(&self) -> HealthStatus {
    ///         match self.db.ping().await {
    ///             Ok(()) => HealthStatus::pass(),
    ///             Err(error) => HealthStatus::fail(format!("db unreachable: {error}")),
    ///         }
    ///     }
    /// }
    ///
    /// Host::builder()
    ///     .add_health_check_async(DatabaseProbe { db })
    ///     .build()
    /// ```
    pub fn add_health_check_async<C: IHealthCheck + 'static>(self, check: C) -> Self {
        self.add_health_check_async_with_timeout(check, DEFAULT_HEALTH_CHECK_TIMEOUT)
    }

    /// Register an async health check with an explicit per-check timeout.
    ///
    /// Timeouts are independent: a slow check fails only itself while the other
    /// probes still report their real status in the same response.
    pub fn add_health_check_async_with_timeout<C: IHealthCheck + 'static>(
        self,
        check: C,
        timeout: Duration,
    ) -> Self {
        self.health_registry
            .register_async(Arc::new(check), timeout);
        self
    }

    /// Build the host, reporting configuration problems as `Err` instead of
    /// panicking.
    ///
    /// This is the non-panicking form of [`HostBuilder::build`]. Production
    /// guards (missing/weak JWT secret, wildcard CORS), a failing DI provider
    /// build, invalid localization resources and inconsistent route/handler
    /// metadata all come back as [`Error::Di`] carrying the same operator-facing
    /// message [`HostBuilder::build`] panics with.
    ///
    /// ```ignore
    /// match Host::builder().mode(AppMode::Production).try_build() {
    ///     Ok(host) => host.run().await?,
    ///     Err(error) => eprintln!("invalid configuration: {error}"),
    /// }
    /// ```
    pub fn try_build(self) -> Result<Host> {
        let prepared = self.prepare()?;
        let PreparedHost {
            builder,
            svc,
            provider_options,
            options,
            endpoints,
            localizer,
            handler_cache,
            shutdown_token,
        } = prepared;
        let provider = svc
            .build_with(provider_options)
            .map_err(build_provider_error)?;
        builder.assemble(
            provider,
            handler_cache,
            options,
            endpoints,
            localizer,
            shutdown_token,
        )
    }

    /// Build the host, awaiting rust-dix's `build_async` for async singletons.
    ///
    /// Use this instead of [`HostBuilder::try_build`] when the service
    /// collection contains `async_*` registrations (for example rust-ef's
    /// `add_dbcontext_async`). Async singletons are initialised dependency-first
    /// by rust-dix before the host is assembled; failures surface as `Error::Di`.
    ///
    /// ```ignore
    /// let host = Host::builder()
    ///     .register(|svc| svc.add_dbcontext_async(|options| { /* ... */ }))
    ///     .try_build_async()
    ///     .await?;
    /// ```
    pub async fn try_build_async(self) -> Result<Host> {
        let prepared = self.prepare()?;
        let PreparedHost {
            builder,
            svc,
            provider_options,
            options,
            endpoints,
            localizer,
            handler_cache,
            shutdown_token,
        } = prepared;
        let provider = svc
            .build_async_with(provider_options)
            .await
            .map_err(build_provider_error)?;
        builder.assemble(
            provider,
            handler_cache,
            options,
            endpoints,
            localizer,
            shutdown_token,
        )
    }

    fn prepare(mut self) -> Result<PreparedHost> {
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
        for modifier in std::mem::take(&mut self.options_modifiers) {
            modifier(&mut options);
        }

        // XR-2: resolve Jwt.SecretFile into Jwt.Secret (env/code secret wins; fail fast).
        crate::auth_jwt::apply_jwt_secret_file(&mut options)?;

        let default_culture = if options.culture.default.trim().is_empty() {
            "en".to_string()
        } else {
            options.culture.default.clone()
        };
        let localization = self
            .localization
            .take()
            .unwrap_or_else(LocalizationOptions::none);
        let localizer: Arc<dyn IStringLocalizer> = Arc::new(
            StringLocalizer::new(default_culture, &localization)
                .map_err(|error| Error::Di(format!("Invalid localization resources: {error}")))?,
        );
        let endpoints: Arc<dyn IEndpointCatalog> = Arc::new(EndpointCatalog::from_inventory());

        let mut svc = ServiceCollection::from_injected();
        // WEBX-01: expose the resolved signing secret to application service
        // factories before their closures run. Only a non-empty secret is
        // registered: an empty `JwtSecret` would let a custom issuer sign with
        // `""`, which is the failure mode this registration exists to prevent.
        if !options.jwt.secret.trim().is_empty() {
            svc = svc.instance(Arc::new(JwtSecret::new(options.jwt.secret.clone())));
        }
        for cfg in std::mem::take(&mut self.service_configs) {
            svc = cfg(svc, &options);
        }

        // Named outbound clients collected by `HostBuilder::add_http_client`.
        // `try_add*` keeps an application-registered registry/factory.
        if !self.http_clients.is_empty() {
            let mut registry = webx_client::HttpClientRegistry::new();
            for (name, client) in std::mem::take(&mut self.http_clients) {
                registry.insert(name, client);
            }
            tracing::info!(
                "[Host] {} outbound HTTP client(s) registered",
                registry.len()
            );
            let registry = Arc::new(registry);
            let factory: Arc<dyn webx_client::IHttpClientFactory> = registry.clone();
            let registry_for_di = Arc::clone(&registry);
            svc = svc.try_add_singleton::<webx_client::HttpClientRegistry>(move |_| {
                Arc::clone(&registry_for_di)
            });
            svc = svc.try_add_singleton::<dyn webx_client::IHttpClientFactory>(move |_| {
                Arc::clone(&factory)
            });
        }

        // WEBX-2: the default JWT issuer is a host service, so a login endpoint
        // injects `Arc<dyn ITokenIssuer>` instead of hand-rolling signing. It is
        // registered through `try_add*`: an application registration of its own
        // issuer (added above) wins over this default. The lifetime comes from
        // `Jwt.ExpirationMinutes` (PascalCase in appsettings, default 60); `0`
        // falls back to the default instead of minting already-expired tokens.
        if self.auth_enabled && !options.jwt.secret.trim().is_empty() {
            let secret = options.jwt.secret.clone();
            let lifetime_minutes = match options.jwt.expiration_minutes {
                0 => 60,
                minutes => minutes,
            };
            let lifetime = Duration::from_secs(lifetime_minutes.saturating_mul(60));
            svc = svc.try_add_singleton::<dyn ITokenIssuer>(move |_| {
                Arc::new(JwtTokenIssuer::from_secret(secret.as_bytes(), lifetime))
            });
        }

        // WEBX-3: the default password hasher is a host service as well, so a
        // login handler injects `Arc<dyn IPasswordHasher>` and keeps no crypto
        // code. `try_add*`: an application registration (e.g. custom cost
        // parameters via `HostBuilder::use_password_hasher`) wins.
        #[cfg(feature = "password-argon2")]
        {
            svc = svc.try_add_singleton::<dyn webx_core::security::IPasswordHasher>(|_| {
                Arc::new(crate::password_argon2::Argon2PasswordHasher::default())
            });
        }

        // The compiled handler registry is a host service: `add_mediator`
        // resolves it from DI, and the HTTP path reads it from DispatchRuntime.
        let handler_cache = Arc::new(webx_core::route::scan::HandlerCache::build());
        svc = svc.instance(Arc::clone(&handler_cache));

        // Host-provided defaults use `try_add*`: rust-dix 0.8+ resolves the *last*
        // registration, so an earlier user registration must win over these.
        let health_registry = Arc::clone(&self.health_registry);
        svc = svc.try_add(move |_| Arc::clone(&health_registry));

        // Endpoint metadata and localization are host services, not globals.
        let endpoints_for_di = Arc::clone(&endpoints);
        svc = svc.try_add_singleton::<dyn IEndpointCatalog>(move |_| Arc::clone(&endpoints_for_di));
        let localizer_for_di = Arc::clone(&localizer);
        svc = svc.try_add_singleton::<dyn IStringLocalizer>(move |_| Arc::clone(&localizer_for_di));

        // Host shutdown token: registered before the provider is built so
        // long-lived responses can inject it. Cancelled by every shutdown path
        // (signal, `ServerHandle::shutdown`, `IHost::stop`).
        let shutdown_token = CancellationToken::new();
        svc = svc.instance(Arc::new(ShutdownToken::new(shutdown_token.clone())));

        let provider_options = if self.lenient_scope_validation {
            rust_dix::ServiceProviderOptions::lenient()
        } else {
            rust_dix::ServiceProviderOptions::default()
        };
        Ok(PreparedHost {
            builder: self,
            svc,
            provider_options,
            options,
            endpoints,
            localizer,
            handler_cache,
            shutdown_token,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble(
        self,
        provider: Arc<ServiceProvider>,
        handler_cache: Arc<webx_core::route::scan::HandlerCache>,
        options: AppOptions,
        endpoints: Arc<dyn IEndpointCatalog>,
        localizer: Arc<dyn IStringLocalizer>,
        shutdown_token: CancellationToken,
    ) -> Result<Host> {
        // Per-host dispatch runtime (provider + handler registry).
        let dispatch_runtime = Arc::new(DispatchRuntime::new(Arc::clone(&provider), handler_cache));

        // Upload spooling and body limits come from the same merged config.
        let body_limits = Arc::new(webx_engine::BodyLimits::from_options(&options));
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
        // Security headers are applied by the engine's finalizer (static
        // HeaderName/HeaderValue, no per-request middleware allocation). The
        // public `SecurityHeadersMiddleware` remains available for custom
        // pipelines, but the host does not add it a second time.
        pipeline.add_middleware(Arc::new(crate::request_id::RequestIdMiddleware::new()));

        // Metrics wrap CORS so preflight responses (which short-circuit in the
        // CORS stage) are observable, and wrap rate limiting/auth so rejected
        // requests are counted with their real status.
        if options.metrics.enabled {
            tracing::info!("[Host] Metrics enabled at GET /metrics");
            pipeline.add_middleware(Arc::new(MetricsMiddleware::new(Arc::clone(&http_metrics))));
        }

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
        // Validate the configuration the middleware will actually use. The
        // previous guard checked `options.cors` only, so
        // `use_cors(CorsConfig::default())` could ship `ACAO: *` in Production.
        if self.mode == AppMode::Production && cors.origins.iter().any(|o| o == "*") {
            return Err(Error::Di(
                "Production forbids CORS origin '*'. Set explicit origins via Cors.Origins or APP__Cors__Origins."
                    .to_string(),
            ));
        }
        if !cors.origins.is_empty() {
            pipeline.add_middleware(Arc::new(CorsMiddleware::new(cors)));
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

        let mut auth_schemes = self.auth_schemes;
        if self.auth_enabled {
            let secret = options.jwt.secret.clone();
            if !secret.is_empty() {
                let jwt_auth = Arc::new(JwtAuth::new(
                    DecodingKey::from_secret(secret.as_bytes()),
                    Validation::default(),
                ));
                // WEBX-2: when the application registered an `ISessionStore`,
                // verified tokens are additionally checked for revocation.
                // Without one this stays the 1.1 verify-only behaviour.
                let session_store = provider.get_all::<dyn ISessionStore>().into_iter().next();
                match &session_store {
                    Some(_) => tracing::debug!(
                        "[Host] JWT revocation enabled (a dyn ISessionStore is registered)"
                    ),
                    None => tracing::debug!(
                        "[Host] JWT revocation disabled (no dyn ISessionStore registered; \
                         use HostBuilder::use_session_store)"
                    ),
                }
                auth_schemes.push(Arc::new(JwtScheme::new(jwt_auth, session_store)));
                init_jwt_secret(&secret);
            } else {
                tracing::warn!(
                    "add_authentication() enabled but no JWT secret configured. Set Jwt.Secret in appsettings.json, JWT_SECRET, or APP__Jwt__Secret env var."
                );
            }
        }

        if !auth_schemes.is_empty() {
            let middleware = AuthenticationMiddleware::new(auth_schemes);
            tracing::debug!(
                schemes = ?middleware.names(),
                "[Host] authentication schemes enabled"
            );
            pipeline.add_middleware(Arc::new(middleware));
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
            return Err(Error::Di(
                "Production requires a strong JWT secret. Set APP__Jwt__Secret or JWT_SECRET env var (min 32 chars, not a placeholder)."
                    .to_string(),
            ));
        }

        let mut engine_router = webx_engine::Router::new();
        let mut route_count = 0usize;

        crate::diagnostics::validate_route_configuration()?;

        // Build dispatch maps. The TypeId map is authoritative for routes
        // produced by the endpoint macros; the name map remains only as a
        // compatibility fallback for hand-written fixtures.
        let mut dispatch_map: std::collections::HashMap<
            &'static str,
            webx_core::route::scan::RouteDispatchFn,
        > = std::collections::HashMap::new();
        let mut dispatch_by_id: std::collections::HashMap<
            std::any::TypeId,
            webx_core::route::scan::RouteDispatchFn,
        > = std::collections::HashMap::new();

        for dispatch in inventory::iter::<webx_core::route::scan::RouteDispatch> {
            dispatch_map.insert(dispatch.handler_type, dispatch.dispatch);
        }
        for dispatch in inventory::iter::<webx_core::route::scan::RouteDispatchById> {
            dispatch_by_id.insert(dispatch.req_type_id, dispatch.dispatch);
        }

        // Collect dynamic authorizers once for the whole host. A dynamic
        // endpoint without one used to fall open (authenticated-only); make
        // that a startup error instead of a silent permission downgrade.
        let authorizers = crate::authz::collect_authorizers(provider.as_ref());
        let has_dynamic_routes =
            inventory::iter::<RouteEntry>().any(|entry| entry.authorization_dynamic);
        if has_dynamic_routes && authorizers.is_none() {
            return Err(Error::Di(
                "A route declares `#[authorize(dynamic)]` but no IDynamicAuthorizer is \
                 registered. Register one with `instance::<dyn IDynamicAuthorizer>(...)` \
                 or remove the dynamic declaration."
                    .to_string(),
            ));
        }

        // Direct dispatch is observationally identical only when no typed
        // pipeline behavior exists for the process; the global count is a
        // conservative gate (a behavior registered by another host disables the
        // fast path here too).
        let direct_dispatch_enabled = webx_core::route::ext::typed_pipeline_count() == 0;
        let mut direct_by_pair: std::collections::HashMap<
            (std::any::TypeId, std::any::TypeId),
            webx_core::route::scan::HandlerDispatchFn,
        > = std::collections::HashMap::new();
        if direct_dispatch_enabled {
            for dispatch in inventory::iter::<webx_core::route::scan::HandlerDispatch> {
                let key = (dispatch.req_type_id, dispatch.rsp_type_id);
                if direct_by_pair.insert(key, dispatch.dispatch).is_some() {
                    return Err(Error::Di(format!(
                        "duplicate direct handler registration for request type '{}'; \
                         each (request, response) pair may have exactly one #[handler]",
                        dispatch.req_type_name
                    )));
                }
            }
        }

        for entry in inventory::iter::<RouteEntry> {
            route_count += 1;
            let dispatch_fn = dispatch_by_id
                .get(&entry.req_type_id)
                .copied()
                .or_else(|| dispatch_map.get(entry.handler_type).copied());
            let stub = Arc::new(StubEndpoint {
                method: entry.method.as_str(),
                path: entry.path,
                handler_type: entry.handler_type,
                dispatch_fn,
                authorization: entry.authorization,
                authorization_dynamic: entry.authorization_dynamic,
                authorizers: authorizers.clone(),
                max_body_size: entry.max_body_size,
            });
            let direct = entry
                .rsp_type_id
                .and_then(|rsp| direct_by_pair.get(&(entry.req_type_id, rsp)).copied());
            let engine_target: Arc<dyn webx_engine::RouteTarget> = match direct {
                Some(dispatch) => Arc::new(crate::endpoint::DirectRouteTarget::new(
                    Arc::clone(&stub),
                    dispatch,
                )),
                None => Arc::new(webx_engine::EndpointTarget::new(
                    Arc::clone(&stub) as Arc<dyn IEndpoint>
                )),
            };
            engine_router.try_insert(entry.method, entry.path, engine_target)?;
        }

        tracing::info!(
            routes = route_count,
            direct = direct_by_pair.len(),
            typed_behaviors = webx_core::route::ext::typed_pipeline_count(),
            "engine dispatch inventory"
        );

        let openapi_spec = generate_openapi_spec(
            "Rust WebX API",
            env!("CARGO_PKG_VERSION"),
            localizer.as_ref(),
        );
        if self.mode == AppMode::Development {
            let openapi_bytes = serde_json::to_vec(&openapi_spec).unwrap_or_default();
            let openapi_json: Arc<dyn IEndpoint> = Arc::new(StaticJsonEndpoint::new(openapi_bytes));
            engine_router.try_insert(
                HttpMethod::Get,
                "/api/openapi.json",
                Arc::new(webx_engine::EndpointTarget::new(openapi_json)),
            )?;
            let openapi_html: Arc<dyn IEndpoint> =
                Arc::new(StaticHtmlEndpoint { body: APIUI_HTML });
            engine_router.try_insert(
                HttpMethod::Get,
                "/api/openapi.html",
                Arc::new(webx_engine::EndpointTarget::new(openapi_html)),
            )?;
        }

        // Health check endpoints (RFC 8407 application/health+json)
        // Probes are evaluated on each request (not baked at build time).
        let health_registry = Arc::clone(&self.health_registry);
        let health_endpoint: Arc<dyn IEndpoint> =
            Arc::new(HealthCheckEndpoint::new(Arc::clone(&health_registry)));
        engine_router.try_insert(
            HttpMethod::Get,
            "/health",
            Arc::new(webx_engine::EndpointTarget::new(Arc::clone(
                &health_endpoint,
            ))),
        )?;
        engine_router.try_insert(
            HttpMethod::Get,
            "/healthz",
            Arc::new(webx_engine::EndpointTarget::new(health_endpoint)),
        )?;

        // /health/live: liveness (process alive → pass, no probe queries)
        let live_body =
            serde_json::to_vec(&serde_json::json!({"status":"pass"})).unwrap_or_default();
        let live_endpoint: Arc<dyn IEndpoint> = Arc::new(StaticJsonEndpoint {
            body: live_body,
            content_type: "application/health+json",
        });
        engine_router.try_insert(
            HttpMethod::Get,
            "/health/live",
            Arc::new(webx_engine::EndpointTarget::new(live_endpoint)),
        )?;

        // /health/ready: readiness (queries registry, same as /health)
        let ready_endpoint: Arc<dyn IEndpoint> =
            Arc::new(HealthCheckEndpoint::new(health_registry));
        engine_router.try_insert(
            HttpMethod::Get,
            "/health/ready",
            Arc::new(webx_engine::EndpointTarget::new(ready_endpoint)),
        )?;

        if options.metrics.enabled {
            let metrics_endpoint: Arc<dyn IEndpoint> = Arc::new(MetricsEndpoint::new(http_metrics));
            engine_router.try_insert(
                HttpMethod::Get,
                "/metrics",
                Arc::new(webx_engine::EndpointTarget::new(metrics_endpoint)),
            )?;
        }

        if self.mode == AppMode::Development {
            let version = env!("CARGO_PKG_VERSION");
            tracing::info!("");
            tracing::info!("  ----------------------------------------------------------------");
            tracing::info!("    Rust WebX Framework v{}", version);
            tracing::info!("  ----------------------------------------------------------------");
            tracing::info!("    App:      {}", options.app.name);
            tracing::info!(
                "    CORS:     {}",
                if options.cors.origins.is_empty() {
                    "disabled (no origins configured)"
                } else {
                    "enabled"
                }
            );
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

        if let Some(spa) = spa_fallback.clone() {
            let fallback: Arc<dyn IEndpoint> =
                Arc::new(crate::endpoint::SpaFallbackEndpoint::new(spa));
            engine_router.set_fallback(Arc::new(webx_engine::EndpointTarget::new(fallback)));
        }

        let error_writers = Arc::new(self.error_writers);

        // Build the 2.0 engine state. TLS is attached per run for HTTPS
        // listeners; the common assemble path keeps it out of `Shared`.
        let mut engine_config = webx_engine::EngineConfig::from_options(&options);
        engine_config.mode = self.mode;
        let (engine_shutdown, _) = tokio::sync::watch::channel(false);
        let engine = Arc::new(webx_engine::Shared {
            config: Arc::new(engine_config),
            router: Arc::new(engine_router),
            pipeline: Arc::new(webx_engine::Pipeline::new(pipeline.stages().to_vec())),
            dispatch: Arc::clone(&dispatch_runtime),
            body_limits: Arc::clone(&body_limits),
            error_writers: Arc::clone(&error_writers),
            tls: None,
            shutdown: engine_shutdown,
            shutdown_token: CancellationToken::new(),
            immediate_shutdown: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            immediate_watch: tokio::sync::watch::channel(false).0,
            started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        // Resolve all registered hosted services from the DI container.
        // These will be started when `run()` is called and stopped on shutdown.
        //
        // Sort by `order()` (stable): equal orders keep the DI registration
        // order, so the historical registration-order behavior is unchanged and
        // `add_hosted_service_ordered` can move a service earlier/later.
        let mut hosted_services: Vec<Arc<dyn IHostedService>> =
            provider.get_all::<dyn IHostedService>();
        hosted_services.sort_by_key(|service| service.order());

        Ok(Host {
            provider,
            dispatch_runtime,
            options,
            endpoints,
            localizer,
            engine,
            spa_source,
            shutdown: Arc::new(tokio::sync::watch::channel(false).0),
            hosted_services: Arc::new(hosted_services),
            hosted_stopped: Arc::new(tokio::sync::OnceCell::new()),
            shutdown_token,
            state: AtomicU8::new(HostState::Built.load()),
        })
    }

    /// Build the host, panicking on configuration errors (fail fast at startup).
    ///
    /// Equivalent to [`HostBuilder::try_build`] followed by a panic that keeps
    /// the historical message, so a misconfigured deployment stops the process
    /// instead of serving with weakened guards. Call `try_build` directly when
    /// the caller wants to handle the error itself.
    pub fn build(self) -> Host {
        match self.try_build() {
            Ok(host) => host,
            // `Error::Di` carries the operator-facing text without a variant
            // prefix, keeping the panic message identical to pre-0.10 builds.
            Err(error) => panic!("{}", build_failure_message(&error)),
        }
    }
}

/// Map a rust-dix provider build failure to the host `Error::Di` form.
fn build_provider_error(error: rust_dix::RdiError) -> Error {
    Error::Di(format!(
        "Failed to build ServiceProvider: {}. Check your DI registrations.",
        error
    ))
}

/// Operator-facing text of a `try_build` failure, without the `Error` variant
/// prefix (the `build()` panic message must not change in 0.10).
fn build_failure_message(error: &Error) -> String {
    match error {
        Error::Di(message) => message.clone(),
        other => other.to_string(),
    }
}

/// Handle for a running server, allowing programmatic graceful shutdown.
pub struct ServerHandle {
    shutdown: ShutdownSignal,
    shutdown_token: CancellationToken,
}

impl ServerHandle {
    /// Signal the server to stop accepting new connections and begin
    /// draining existing ones.
    ///
    /// The signal is stored in a `watch` channel, so calling this before the
    /// server's accept loops start is not lost. The host shutdown token is
    /// cancelled at the same time, which lets long SSE responses end with a
    /// `server_shutdown` event instead of waiting for the drain timeout.
    pub fn shutdown(self) {
        trigger_shutdown(&self.shutdown, &self.shutdown_token);
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

    /// Dispose the DI provider after the server and hosted services stopped.
    ///
    /// rust-dix releases singletons/root-scoped instances in reverse creation
    /// order, runs cleanup closures in reverse registration order and marks the
    /// provider disposed; later resolution returns `ProviderDisposed`. The call
    /// is idempotent, so a second shutdown is harmless.
    fn dispose_di(&self) {
        self.provider.dispose();
        tracing::debug!("[Host] DI provider disposed");
    }

    /// Immediate, non-graceful teardown used when a supervising future is
    /// dropped. Normal shutdowns use [`Host::stop`] or `ServerHandle::shutdown`.
    fn request_immediate_shutdown(&self) {
        trigger_shutdown(&self.shutdown, &self.shutdown_token);
        self.engine.trigger_immediate_shutdown();
    }

    /// Stop every hosted service exactly once (reverse start order).
    ///
    /// Two callers race here on purpose: the shutdown watcher stops workers as
    /// soon as the signal arrives, and the teardown path calls in after the
    /// listeners finish. The cell makes the second call a no-op instead of
    /// running `stop()` twice on services whose cleanup is not idempotent.
    async fn stop_hosted_services_once(&self) {
        let services = Arc::clone(&self.hosted_services);
        self.hosted_stopped
            .get_or_init(|| async move { stop_hosted_services(&services).await })
            .await;
    }

    /// Spawn the task that stops hosted services the moment shutdown starts.
    ///
    /// Waiting for the listeners to drain first would let workers keep running
    /// (and keep touching databases/caches) while the process is already
    /// closing; cancelling them first is what `BackgroundService` promises.
    /// The returned handle is aborted once teardown is done so a host that ends
    /// without a shutdown signal does not leak the watcher.
    fn spawn_hosted_shutdown_watcher(&self) -> tokio::task::JoinHandle<()> {
        let mut shutdown = self.shutdown.subscribe();
        let stopped = Arc::clone(&self.hosted_stopped);
        let services = Arc::clone(&self.hosted_services);
        tokio::spawn(async move {
            // `wait_for` returns immediately when the signal was already sent.
            if shutdown.wait_for(|signalled| *signalled).await.is_ok() {
                stopped
                    .get_or_init(|| async move { stop_hosted_services(&services).await })
                    .await;
            }
        })
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

        let mut parsed = Vec::with_capacity(urls.len());
        for url in urls {
            let (scheme, addr) = parse_url(&url)?;
            parsed.push((scheme.to_string(), addr));
        }
        self.run_on(parsed).await
    }

    /// Bind every configured address first, then run one engine worker pool per
    /// listener sharing the host's DI, router, pipeline and shutdown signal.
    async fn run_on(&self, urls: Vec<(String, String)>) -> Result<()> {
        if urls.is_empty() {
            return Err(Error::Http("no URLs configured".to_string()));
        }

        let mut listeners = Vec::with_capacity(urls.len());
        for (scheme, addr) in urls {
            if scheme != "http" && scheme != "https" {
                return Err(Error::Http(format!(
                    "Unsupported URL scheme '{scheme}'; use http:// or https://"
                )));
            }
            let listener = std::net::TcpListener::bind(&addr)
                .map_err(|err| Error::Http(format!("failed to bind HTTP on {addr}: {err}")))?;
            listeners.push((scheme, addr, listener));
        }

        let mut tls_configs = Vec::with_capacity(listeners.len());
        for (scheme, _, _) in &listeners {
            if scheme == "https" {
                let tls = &self.options.tls;
                if tls.cert_path.is_empty() || tls.key_path.is_empty() {
                    return Err(Error::Http(
                        "HTTPS URLs require Tls.CertPath and Tls.KeyPath".to_string(),
                    ));
                }
                tls_configs.push(Some(webx_engine::tls::build_server_config(
                    &tls.cert_path,
                    &tls.key_path,
                )?));
            } else {
                tls_configs.push(None);
            }
        }

        if self
            .state
            .compare_exchange(
                HostState::Built.load(),
                HostState::Running.load(),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return Err(Error::State(
                "Host is single-use; build a new Host to run again".to_string(),
            ));
        }
        let mut run_guard = HostRunGuard::new(self);

        if let Err(err) = start_hosted_services(&self.hosted_services).await {
            // Startup rollback already stopped the services that did start; the
            // provider is still valid, so leave the host retryable instead of
            // disposing it and turning every later construction into a 500.
            self.state.store(HostState::Built.load(), Ordering::Release);
            run_guard.disarm();
            return Err(err);
        }

        install_shutdown_handler(Arc::clone(&self.shutdown), self.shutdown_token.clone());
        let signal_listener =
            spawn_async_shutdown_listener(Arc::clone(&self.shutdown), self.shutdown_token.clone());
        let hosted_watcher = self.spawn_hosted_shutdown_watcher();

        let engine_for_shutdown = (*self.engine).clone();
        let mut host_shutdown = self.shutdown.subscribe();
        let shutdown_bridge = tokio::spawn(async move {
            if host_shutdown.wait_for(|value| *value).await.is_ok() {
                engine_for_shutdown.trigger_shutdown();
            }
        });

        // The reverse direction matters on engine-internal failure: the engine
        // signals its own shutdown before it drains. Mirror that into the host
        // watch so hosted services stop before in-flight requests are joined.
        let engine_shutdown_watch = self.engine.shutdown.subscribe();
        let host_shutdown_signal = Arc::clone(&self.shutdown);
        let host_shutdown_token = self.shutdown_token.clone();
        let engine_to_host_shutdown = tokio::spawn(async move {
            let mut watch = engine_shutdown_watch;
            if watch.wait_for(|value| *value).await.is_ok() {
                trigger_shutdown(&host_shutdown_signal, &host_shutdown_token);
            }
        });

        let mut engine_listeners = Vec::with_capacity(listeners.len());
        for ((_scheme, _addr, raw), tls) in listeners.into_iter().zip(tls_configs) {
            engine_listeners.push(match tls {
                Some(tls) => webx_engine::Listener::tls(raw, tls),
                None => webx_engine::Listener::plain(raw),
            });
        }

        // One Shared, one worker pool, one global connection governor for all
        // URLs. TLS is per Listener; `config.tls` must not leak one certificate
        // onto another listener.
        let mut shared = (*self.engine).clone();
        shared.shutdown_token = self.shutdown_token.clone();
        let mut config = (*shared.config).clone();
        config.tls = None;
        shared.config = Arc::new(config);
        shared.tls = None;

        let run_result = webx_engine::Server::new(Arc::new(shared))
            .run_listeners(engine_listeners)
            .await;

        self.stop_hosted_services_once().await;
        shutdown_bridge.abort();
        engine_to_host_shutdown.abort();
        hosted_watcher.abort();
        signal_listener.abort();

        if run_result.is_ok() {
            self.dispose_di();
            self.state
                .store(HostState::Disposed.load(), Ordering::Release);
        } else {
            // A join timeout can leave a blocking worker alive; disposing the
            // provider here would let that worker resolve a disposed container.
            // Keep the provider and make the host permanently unusable.
            self.state
                .store(HostState::Failed.load(), Ordering::Release);
        }
        run_guard.disarm();
        run_result
    }

    /// Start the server at a single explicit address.
    ///
    /// This is the engine path: bind once, start hosted services, map both
    /// shutdown directions (host↔engine), then let the shared worker pool
    /// accept/drain.
    pub async fn run_at(&self, addr: &str) -> Result<()> {
        let (scheme, bind_addr) = match parse_url(addr) {
            Ok((scheme, bind_addr)) => (scheme.to_string(), bind_addr),
            Err(_) => ("http".to_string(), addr.to_string()),
        };
        self.run_on(vec![(scheme, bind_addr)]).await
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
            shutdown_token: self.shutdown_token.clone(),
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
        trigger_shutdown(&self.shutdown, &self.shutdown_token);
        Ok(())
    }
}

fn trigger_shutdown(shutdown: &ShutdownSignal, token: &CancellationToken) {
    // `send_replace` stores the value unconditionally. `send` would fail while
    // no receiver exists (e.g. `handle.shutdown()` called before `run()`
    // subscribes) and silently drop the signal — the exact bug this type
    // replaces `Notify` for.
    shutdown.send_replace(true);
    // Wake long-lived responses (SSE) so they can close cleanly before the
    // drain deadline instead of being aborted.
    token.cancel();
}

fn install_shutdown_handler(shutdown: ShutdownSignal, token: CancellationToken) {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock, Weak};

    /// A host that is alive: its watch signal plus the token registered in DI.
    type LiveHost = (Weak<tokio::sync::watch::Sender<bool>>, CancellationToken);

    static HANDLERS: OnceLock<Mutex<Vec<LiveHost>>> = OnceLock::new();
    static CTRL_COUNT: AtomicUsize = AtomicUsize::new(0);
    static INSTALLED: AtomicBool = AtomicBool::new(false);

    let handlers = HANDLERS.get_or_init(|| Mutex::new(Vec::new()));
    {
        let mut registered = handlers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Drop hosts that have already exited before adding this one.
        registered.retain(|(weak, _)| weak.strong_count() > 0);
        registered.push((Arc::downgrade(&shutdown), token));
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
            for (weak, token) in registered.iter() {
                if let Some(shutdown) = weak.upgrade() {
                    trigger_shutdown(&shutdown, token);
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

fn spawn_async_shutdown_listener(
    shutdown: ShutdownSignal,
    token: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let cancelled = token.clone();
    tokio::spawn(async move {
        // Exit if shutdown was triggered by another source (another host, an
        // explicit handle.shutdown(), or the server loop ending). Otherwise
        // each run() leaks a listener that holds the watch channel alive for
        // the entire process lifetime — a problem for tests and embedded
        // multi-host deployments.
        tokio::select! {
            _ = wait_for_shutdown_signal() => {
                tracing::info!("Async shutdown signal received, draining connections...");
                trigger_shutdown(&shutdown, &token);
            }
            _ = cancelled.cancelled() => {
                tracing::debug!("Shutdown listener stopped: already shutting down");
            }
        }
    })
}

async fn start_hosted_services(services: &[Arc<dyn IHostedService>]) -> Result<()> {
    if services.is_empty() {
        return Ok(());
    }
    tracing::info!("Starting {} hosted service(s)...", services.len());
    let mut started = 0usize;
    let mut timed_out: Option<usize> = None;
    let mut failure: Option<webx_core::error::Error> = None;

    for svc in services {
        // No ambient DI scope is created here: hosted services own their scope
        // via an injected `Arc<dyn ScopeFactory>` (rust-dix built-in) and dispose
        // it themselves, so scoped dependencies never outlive the service run.
        let attempt = tokio::time::timeout(
            std::time::Duration::from_secs(HOSTED_START_TIMEOUT_SECS),
            svc.start(),
        )
        .await;
        match attempt {
            Ok(Ok(())) => started += 1,
            Ok(Err(err)) => {
                failure = Some(err);
                break;
            }
            Err(_) => {
                timed_out = Some(started);
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
            // A timed-out `start` may have spawned its loop before blocking;
            // include it in the best-effort rollback.
            if let Some(index) = timed_out {
                if let Some(svc) = services.get(index) {
                    stop_hosted_service(index, svc).await;
                }
            }
            // Roll back the services that did start, newest first, so a failed
            // startup does not leave pools/loops running.
            for (index, svc) in services[..started].iter().enumerate().rev() {
                stop_hosted_service(index, svc).await;
            }
            Err(err)
        }
    }
}

async fn stop_hosted_services(services: &[Arc<dyn IHostedService>]) {
    if services.is_empty() {
        return;
    }
    tracing::info!("Stopping {} hosted service(s)...", services.len());
    for (index, svc) in services.iter().enumerate().rev() {
        stop_hosted_service(index, svc).await;
    }
    tracing::info!("All hosted services stopped.");
}

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

async fn stop_hosted_service(index: usize, svc: &Arc<dyn IHostedService>) {
    match tokio::time::timeout(
        std::time::Duration::from_secs(HOSTED_STOP_TIMEOUT_SECS),
        svc.stop(),
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
