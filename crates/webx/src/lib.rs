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
    ContentResult, FileResult, IActionResult, JsonResult, NoContentResult, Response,
    StatusCodeResult,
};
pub use webx_core::app::IHost;
pub use webx_core::auth::{
    IAuthenticationHandler, IAuthorizationPolicy, IClaims, IDynamicAuthorizer, IdentityUser,
    RequestUser,
};
pub use webx_core::cache::{
    cache_ext::DistributedCacheExtensions,
    options::DistributedCacheEntryOptions,
    trait_def::{CacheError, IDistributedCache},
};
pub use webx_core::config::{
    bind_config, bind_root, load_appsettings, AppOptions, AppSection, CorsSection, CultureSection,
    IAppOptions, JwtSection, MetricsSection, RateLimitSection, TlsSection,
};
pub use webx_core::dispatch_runtime::DispatchRuntime;
pub use webx_core::error::{ActionResult, Error, Result};
pub use webx_core::form::{
    sanitize_file_name, set_spool_root, spool_root, FormField, FormFile, FormFileBuilder,
    MultipartForm, SPOOL_FILE_PREFIX,
};
pub use webx_core::handler::{IEventHandler, IHostedService, IRequestHandler};
pub use webx_core::hosted::{
    BackgroundService, BackgroundServiceHost, CancellationToken, PeriodicBackgroundService,
};
/// Background worker helpers: `BackgroundService`, `delay`, `CancellationToken`.
pub use webx_core::hosted as background;
pub use webx_core::http::{
    content_disposition_value, read_json_body, write_json_response, ByteRange, Disposition,
    FileBody, FileSource, HttpStatus, IClaimsExt, IHttpContext, IHttpRequest, IHttpResponse,
    ResponseBody, SeekableReader, STREAM_BUF_SIZE,
};
pub use webx_core::localization::{
    IStringLocalizer, LocalizationOptions, LocalizedString, StringLocalizer, TextRef,
};
pub use webx_core::mediator::dispatch;
pub use webx_core::mediator::{IEventRequest, IMediator, IRequest};
pub use webx_core::middleware::{IMiddleware, MiddlewareSlot};
pub use webx_core::mode::AppMode;
pub use webx_core::operation::OperationResult;
pub use webx_core::pagination::{PagedRequest, PagedResult};
pub use webx_core::paths::{
    app_base, framework_root, looks_like_app_base, looks_like_framework_root,
};
pub use webx_core::pipeline::{IPipelineBehavior, Next};
pub use webx_core::problem::{FieldError, ProblemDetails};
pub use webx_core::request_context::RequestContext;
pub use webx_core::route::catalog::{
    EndpointCatalog, EndpointDescriptor, EndpointInfo, IEndpointCatalog,
};
pub use webx_core::route::diagnostics::{
    duplicate_handlers, format_route_diagnostics, orphan_handlers, orphan_route_details,
    orphan_routes, route_snapshots,
};
pub use webx_core::routing::{HttpMethod, IEndpoint, IRouter, RouteMeta};

// --- DI extensions ---
pub use webx_core::route::binding;
pub use webx_core::route::binding::RequestModel;
pub use webx_core::route::ext::{is_mediator_active, should_scan_endpoints, IServiceCollectionExt};
pub use webx_core::route::scan::{
    AuthorizeRule, BoxedDispatch, BoxedDispatchFn, BoxedDispatchFuture, HandlerCache, HandlerEntry,
    HandlerRegistration, HandlerRegistry, ParamMeta, RequestParamEntry, RouteDispatch,
    RouteDispatchFn, RouteEntry,
};

// --- HTTP layer ---
pub use webx_host::auth_jwt::{init_jwt_secret, jwt_middleware, jwt_secret, JwtAuth, JwtClaims};
pub use webx_host::authz::collect_authorizers;
pub use webx_host::authz::resource_auth_middleware;
pub use webx_host::authz::{AuthorizerSet, ResourceAuthorization};
pub use webx_host::compression::{compress_gzip, CompressionConfig, CompressionMiddleware};
pub use webx_host::context::{HttpContext, HttpRequest, HttpResponse};
pub use webx_host::cors::{CorsConfig, CorsMiddleware};
pub use webx_host::diagnostics::log_startup_diagnostics;
pub use webx_host::endpoint::{
    ControllerEndpoint, RequestEndpoint, StaticHtmlEndpoint, StaticJsonEndpoint,
};
pub use webx_host::health::{HealthCheckEntry, HealthCheckFn, HealthCheckRegistry, HealthStatus};
pub use webx_host::memory_cache::MemoryCache;
pub use webx_host::pipeline::{HandlerFn, MiddlewarePipeline};
pub use webx_host::rate_limit::{RateLimitMiddleware, RateLimiter};
pub use webx_host::request_id::RequestIdMiddleware;
pub use webx_host::request_tracing::RequestTracing;
pub use webx_host::router::Router;
pub use webx_host::security_headers::SecurityHeadersMiddleware;
pub use webx_host::server::{Host, HostAppBuilder, HostBuilder, Server, ServerHandle};
#[cfg(feature = "testing")]
pub use webx_host::testing::{free_port, spawn, spawn_with_health, wait_until_ready, TestServer};
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
#[doc(hidden)]
pub use webx_macros::{__webx_category, __webx_description, __webx_display};
pub use webx_macros::{
    authorize, category, delete, description, display, embed_assets, endpoint, get, handler, main,
    post, put, WebxRequestMeta,
};

// --- Re-export rust_dix (for manual registration, #[inject] auto-registration, and module blocks) ---
pub use rust_dix;
pub use rust_dix::inject;
pub use rust_dix::ScopeFactory;
pub use rust_dix_macros;
pub use rust_dix_macros::{module, Inject};

// --- Re-export common dependencies ---
pub use async_trait::async_trait;
pub use hyper;
/// Compile-time registration used by the derive macros.
pub use inventory;
pub use serde;
pub use serde_json;
pub use tokio;
