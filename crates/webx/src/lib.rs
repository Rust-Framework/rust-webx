//! Umbrella crate for the rust-webx framework (package `rust-webx`, imported as
//! `webx`).
//!
//! Re-exports the whole framework so applications need a single dependency:
//!
//! ```toml
//! [dependencies]
//! rust-webx = "1.1"
//! ```
//!
//! ```ignore
//! use webx::*;
//! ```
//!
//! The dependency name is `rust-webx`; the import name is `webx`. The same
//! applies to the sub-crates (`rust-webx-core` → `webx_core`,
//! `rust-webx-host` → `webx_host`, `rust-webx-macros` → `webx_macros`,
//! `rust-webx-spa` → `webx_spa`, `rust-webx-openapi` → `webx_openapi`).

// --- Core traits ---
pub use webx_core::action::{
    ContentResult, EndpointResult, FileResult, IActionResult, JsonResult, NoContentResult,
    Response, StatusCodeResult,
};
pub use webx_core::app::IHost;
pub use webx_core::auth::{
    DeclaredRules, IAuthenticationHandler, IAuthenticationScheme, IAuthorizationPolicy, IClaims,
    IDynamicAuthorizer, IdentityUser, RequestUser,
};
pub use webx_core::cache::{
    cache_ext::DistributedCacheExtensions,
    options::DistributedCacheEntryOptions,
    trait_def::{CacheError, IDistributedCache},
};
pub use webx_core::config::{
    bind_config, bind_root, load_appsettings, try_bind_config, try_bind_root, try_load_appsettings,
    AppOptions, AppSection, ConfigError, ConfigSources, CorsSection, CultureSection, IAppOptions,
    InvalidTokenBehavior, JwtAlgorithm, JwtSection, MetricsSection, RateLimitSection, TlsSection,
};
pub use webx_core::dispatch_runtime::DispatchRuntime;
pub use webx_core::error::{ActionResult, Error, IErrorResponseWriter, Result};
pub use webx_core::form::{
    sanitize_file_name, set_spool_root, spool_root, FormField, FormFile, FormFileBuilder,
    MultipartForm, SPOOL_FILE_PREFIX,
};
pub use webx_core::handler::{IEventHandler, IHostedService, IRequestHandler};
/// Background worker helpers: `BackgroundService`, `delay`, `CancellationToken`.
pub use webx_core::hosted as background;
pub use webx_core::hosted::{
    BackgroundService, BackgroundServiceHost, CancellationToken, PeriodicBackgroundService,
    ShutdownToken,
};
pub use webx_core::http::{
    content_disposition_value, read_json_body, write_json_response, ByteRange, Disposition,
    FileBody, FileSource, HttpStatus, IClaimsExt, IHttpContext, IHttpRequest, IHttpResponse,
    RequestBodyStream, ResponseBody, SeekableReader, STREAM_BUF_SIZE,
};
pub use webx_core::json;
pub use webx_core::localization::{
    IStringLocalizer, LocalizationOptions, LocalizedString, StringLocalizer, TextRef,
};
pub use webx_core::media::{
    IMediaPlaneAdapter, InMemoryMediaPlaneAdapter, MediaSession, MediaStats, MediaTrackInfo,
    MediaTrackKind,
};
pub use webx_core::mediator::dispatch;
pub use webx_core::mediator::{
    dispatch_event_handlers, BoxedEventDispatch, BoxedEventHandlerCall, EventDispatchFn,
    EventDispatcher, EventHandlerCache, EventHandlerCall, EventHandlerFactory,
    EventHandlerRegistration, EventMessage, EventMetrics, EventMetricsSnapshot, EventRegistration,
    IEventPublisher, InMemoryEventPublisher, InMemoryEventWorker, DEFAULT_EVENT_QUEUE_CAPACITY,
};
pub use webx_core::mediator::{IEventRequest, IMediator, IRequest};
pub use webx_core::metrics::{Counter, Gauge, Histogram, IMeter, MemoryMeter, NoopMeter};
pub use webx_core::middleware::{
    BoxedNextFuture, IMiddleware, MiddlewareSlot, NextStage, TimeoutMiddleware,
};
pub use webx_core::mode::AppMode;
pub use webx_core::operation::OperationResult;
pub use webx_core::options_monitor::{IOptionsMonitor, OptionsMonitor};
pub use webx_core::pagination::{PagedRequest, PagedResult};
pub use webx_core::paths::{
    app_base, framework_root, looks_like_app_base, looks_like_framework_root,
};
pub use webx_core::pipeline::{IPipelineBehavior, Next};
pub use webx_core::problem::{FieldError, ProblemDetails};
pub use webx_core::rate_limit::{IRateLimiter, RateLimitDecision};
pub use webx_core::realtime::{
    BackpressurePolicy, ConnectionContext, ConnectionId, ConnectionMeta, ConnectionScope,
    ConnectionStats, IConnection, IConnectionRegistry, IUpgradeSession, IWebSocketHandler,
    InMemoryConnectionRegistry, Presence, PresenceStatus, RealtimeFanoutEvent, RealtimeMessage,
    RealtimeMetrics, RealtimeMetricsSnapshot, SignalingKind, SignalingMessage, Transport,
    TransportKind, TurnCredentials, TurnRestConfig, UpgradedTransport, WebSocketLimits,
    DEFAULT_TURN_TTL_SECONDS, MAX_TURN_TTL_SECONDS,
};
pub use webx_core::request_context::RequestContext;
pub use webx_core::route::catalog::{
    EndpointCatalog, EndpointDescriptor, EndpointInfo, IEndpointCatalog,
};
pub use webx_core::route::diagnostics::{
    duplicate_handlers, format_route_diagnostics, orphan_handlers, orphan_route_details,
    orphan_routes, route_snapshots,
};
pub use webx_core::routing::{HttpMethod, IEndpoint, IRouter, RouteMeta};
pub use webx_core::secrets::{
    ChainSecretStore, EnvSecretStore, FileSecretStore, ISecretStore, SecretError, SecretRef,
    SecretValue,
};
pub use webx_core::security::{
    constant_time_eq, IPasswordHasher, IRefreshTokenStore, ISessionRepository, ISessionStore,
    ITokenIssuer, InMemoryRefreshTokenStore, InMemorySessionRepository, InMemorySessionStore,
    IssuedToken, OneTimeSecret, OneTimeSecretState, RefreshRotation, RefreshTokenRecord,
    RefreshTokenStatus, SessionRecord, TokenClaims,
};
pub use webx_core::storage::{
    FileSystemObjectStore, IObjectStore, InMemoryObjectStore, ObjectMeta, PartEtag, RecordingMeta,
    RecordingSegment, RecordingStatus, StoredObject, UploadId,
};
pub use webx_core::stream::{SseEvent, SseResult, StreamEnd};
pub use webx_core::time::{IClock, SystemClock, TestClock};

// --- DI extensions ---
pub use webx_core::route::binding;
pub use webx_core::route::binding::RequestModel;
pub use webx_core::route::ext::{
    is_mediator_active, should_scan_endpoints, typed_pipeline_count, IServiceCollectionExt,
};
pub use webx_core::route::scan::{
    bind_request, AuthorizeRule, BoxedBindFuture, BoxedBindFutureFn, BoxedDispatch,
    BoxedDispatchFn, BoxedDispatchFuture, DirectResponseEncoder, DirectResponseEncoderFn,
    HandlerCache, HandlerDispatch, HandlerDispatchFn, HandlerEntry, HandlerRegistration,
    HandlerRegistry, ParamMeta, RequestBinder, RequestParamEntry, RouteDispatch, RouteDispatchById,
    RouteDispatchFn, RouteEntry, RouteSource,
};

// --- HTTP layer ---
pub use webx_client::{
    BodySource, HttpClient, HttpClientError, HttpClientOptions, HttpClientRegistry, IHttpClient,
    IHttpClientFactory, OutboundRequest, OutboundResponse, ResponseStream, SseStream,
};
pub use webx_host::auth::{
    ApiKeyClaims, ApiKeyHeader, ApiKeyScheme, AuthMetrics, AuthOutcome, AuthenticationMiddleware,
    IApiKeyValidator, JwtScheme, AUTH_DURATION_METRIC, AUTH_REQUESTS_METRIC,
};
pub use webx_host::auth_basic::{
    BasicClaims, BasicScheme, IBasicCredentialValidator, BASIC_SCHEME_NAME,
};
pub use webx_host::auth_cookie::{
    CookieOptions, IssuedSession, SameSite, SessionCookieScheme, SESSION_COOKIE_SCHEME_NAME,
};
pub use webx_host::auth_dpop::{DpopConfig, DpopScheme, IDpopReplayStore, InMemoryDpopReplayStore};
pub use webx_host::auth_jwks::{
    HttpJwksFetcher, IJwksFetcher, IJwtKeyResolver, Jwk, JwkSet, JwksHealthCheck, JwksResolver,
    ResolvedJwk, StaticKeyResolver, JWKS_MAX_DOCUMENT_BYTES,
};
#[allow(deprecated)]
pub use webx_host::auth_jwt::{
    apply_jwt_secret_file, init_jwt_secret, jwt_middleware, jwt_middleware_with_revocation,
    jwt_secret, JwtAuth, JwtClaims, JwtSecret, JwtTokenIssuer, JwtValidation, JwtValidationBuilder,
    DEFAULT_CLOCK_SKEW_SECONDS, DEFAULT_MAX_TOKEN_BYTES, DEFAULT_TOKEN_LIFETIME,
    MIN_JWT_SECRET_LEN,
};
pub use webx_host::auth_mtls::{IMtlsIdentityResolver, MtlsScheme};
pub use webx_host::authz::collect_authorizers;
#[allow(deprecated)]
pub use webx_host::authz::resource_auth_middleware;
pub use webx_host::authz::{AuthorizerSet, ResourceAuthorization};
pub use webx_host::compression::{compress_gzip, CompressionConfig, CompressionMiddleware};
pub use webx_host::cors::{CorsConfig, CorsMiddleware};
pub use webx_host::csrf::{CsrfConfig, CsrfMiddleware};
pub use webx_host::diagnostics::log_startup_diagnostics;
pub use webx_host::endpoint::{
    ControllerEndpoint, RequestEndpoint, StaticHtmlEndpoint, StaticJsonEndpoint,
};
pub use webx_host::health::{
    HealthCheckEntry, HealthCheckFn, HealthCheckRegistry, HealthStatus, IHealthCheck,
    DEFAULT_HEALTH_CHECK_TIMEOUT,
};
pub use webx_host::introspection::{
    IntrospectionBasicAuth, IntrospectionResponse, TokenIntrospector, TokenRevoker,
};
pub use webx_host::login::{
    default_password_concurrency, verify_password_or_dummy, BoundedPasswordHasher, ILoginThrottle,
    LoginThrottleDecision, MemoryLoginThrottle, PasswordPolicy, ThrottlePolicy, MAX_PASSWORD_BYTES,
};
pub use webx_host::memory_cache::MemoryCache;
#[cfg(feature = "password-argon2")]
pub use webx_host::password_argon2::{Argon2Params, Argon2PasswordHasher};
pub use webx_host::pipeline::{HandlerFn, MiddlewarePipeline};
pub use webx_host::rate_limit::{InMemoryRateLimiter, RateLimitMiddleware, RateLimiter};
pub use webx_host::refresh::{IssuedRefreshToken, RefreshTokenService};
pub use webx_host::request_id::RequestIdMiddleware;
pub use webx_host::request_tracing::RequestTracing;
pub use webx_host::router::Router;
pub use webx_host::security_headers::SecurityHeadersMiddleware;
pub use webx_host::server::{Host, HostAppBuilder, HostBuilder, Server, ServerHandle};
pub use webx_host::session_cache::CachedSessionStore;
#[cfg(feature = "testing")]
pub use webx_host::testing::{free_port, spawn, spawn_with_health, wait_until_ready, TestServer};
pub use webx_host::testing::{ChaosControl, TestHost};
pub use webx_host::timing::TimingMiddleware;

// --- Mediator ---
pub use webx_core::mediator::Mediator;

// --- Web (SPA) ---
pub use webx_spa::SpaMiddleware;

/// Static file hosting, including files compiled into the executable.
///
/// ```ignore
/// // build.rs: webx::builder().web_root("wwwroot").build()
/// #[webx::main(embed)]
/// async fn main() {
///     Host::builder().use_spa("wwwroot").build()
/// }
/// ```
pub mod spa {
    pub use webx_spa::{
        embed_assets, inventory, ContentEncoding, EmbeddedAsset, EmbeddedAssets, SpaMiddleware,
        SpaSource, EMBED_ENV,
    };
}

/// Files compiled into the executable by `#[webx::main(embed)]`, read by path.
///
/// ```ignore
/// let config = webx::assets::read_str("config/default.json");
/// ```
pub mod assets {
    pub use webx_spa::assets::{read, read_str};
}

// --- OpenAPI ---
pub use webx_openapi::{generate_openapi_spec, APIUI_HTML};

// --- Macros ---
pub use webx_macros::__webx_max_body_size;
#[doc(hidden)]
pub use webx_macros::{__webx_authorize, __webx_category, __webx_description, __webx_display};
pub use webx_macros::{
    authorize, category, delete, description, display, embed_assets, endpoint, get, handler, main,
    max_body_size, post, put, WebxRequestMeta,
};

// --- Re-export rust_dix (for manual registration, #[inject] auto-registration, and module blocks) ---
pub use rust_dix;
pub use rust_dix::inject;
pub use rust_dix::ScopeFactory;
pub use rust_dix_macros;
pub use rust_dix_macros::{module, Inject};

// --- Re-export common dependencies ---
pub use async_trait::async_trait;
pub use bytes;
pub use hyper;
/// Compile-time registration used by the derive macros.
pub use inventory;
pub use serde;
pub use serde_json;
pub use tokio;
