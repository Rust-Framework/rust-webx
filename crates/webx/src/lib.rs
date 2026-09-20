//! Umbrella crate for the rust-webx framework (package `rust-webx`, imported as
//! `webx`).
//!
//! Re-exports the whole framework so applications need a single dependency:
//!
//! ```toml
//! [dependencies]
//! rust-webx = "0.5"
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
pub use webx_core::app::IHost;
pub use webx_core::auth::{
    IAuthenticationHandler, IAuthorizationPolicy, IClaims, IDynamicAuthorizer,
};
pub use webx_core::cache::{
    cache_ext::DistributedCacheExtensions,
    options::DistributedCacheEntryOptions,
    trait_def::{CacheError, IDistributedCache},
};
pub use webx_core::config::{
    bind_config, bind_root, load_appsettings, AppOptions, AppSection, CorsSection, IAppOptions,
    JwtSection, MetricsSection, RateLimitSection, TlsSection,
};
pub use webx_core::dispatch_runtime::{dispatch_provider, DispatchRuntime};
pub use webx_core::error::{Error, Result};
pub use webx_core::form::{
    sanitize_file_name, set_spool_root, spool_root, FormDeserializer, FormError, FormField,
    FormFile, FormFileBuilder, MultipartForm, SPOOL_FILE_PREFIX,
};
pub use webx_core::handler::{IClaimsCarrier, IEventHandler, IHostedService, IRequestHandler};
pub use webx_core::http::{
    content_disposition_value, read_json_body, write_json_response, ByteRange, Disposition,
    FileBody, FileSource, FromHttpContext, HttpStatus, IClaimsExt, IHttpContext, IHttpRequest,
    IHttpResponse, Json, ResponseBody, SeekableReader, STREAM_BUF_SIZE,
};
pub use webx_core::mediator::build_pipeline_chain;
pub use webx_core::mediator::{IEventRequest, IMediator, IRequest};
pub use webx_core::middleware::IMiddleware;
pub use webx_core::mode::AppMode;
pub use webx_core::pagination::{PagedRequest, PagedResponse};
pub use webx_core::paths::{
    app_base, framework_root, looks_like_app_base, looks_like_framework_root,
};
pub use webx_core::pipeline::{BoxedNextFn, BoxedPipelineFuture, IPipelineBehavior};
pub use webx_core::problem::{FieldError, ProblemDetails};
pub use webx_core::request_context::RequestContext;
pub use webx_core::route::diagnostics::{
    duplicate_handlers, format_route_diagnostics, orphan_handlers, orphan_route_details,
    orphan_routes, route_snapshots,
};
pub use webx_core::routing::{HttpMethod, IEndpoint, IRouter, RouteMeta};

// --- DI extensions ---
pub use webx_core::route::bind::bind_form_request;
pub use webx_core::route::ext::{is_mediator_active, should_scan_endpoints, IServiceCollectionExt};
pub use webx_core::route::params::try_deserialize_from_params;
#[allow(deprecated)]
pub use webx_core::route::scan::{
    global_provider, set_global_provider, HandlerCache, HandlerEntry, HandlerRegistration,
    HandlerRegistry, ParamMeta, RequestParamEntry, ResponseData, RouteDispatch, RouteDispatchFn,
    RouteEntry,
};

// --- HTTP layer ---
pub use webx_host::auth_jwt::{init_jwt_secret, jwt_middleware, jwt_secret, JwtAuth, JwtClaims};
pub use webx_host::authz::{
    build_resource_policy_from_routes, collect_authorizers, resource_auth_middleware,
    AuthorizerSet, ResourceAuthorization,
};
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

// --- OpenAPI ---
pub use webx_openapi::{generate_openapi_spec, APIUI_HTML};

// --- Macros ---
pub use webx_macros::{
    authorize, claims, delete, embed_assets, endpoint, get, handler, main, post, put,
    WebxRequestMeta,
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
pub use serde;
pub use serde_json;
pub use tokio;

// =========================================================================
// Convenience macro: register_handlers!
//
// **Deprecated for HTTP handlers.** HTTP dispatch uses inventory +
// `HandlerCache` exclusively (`#[handler]` / `#[handler(inject)]`).
// Keep this macro only for non-HTTP Mediator use or manual DI registration.
//
// Generates chained .singleton() calls for handlers that implement Default.
//
// Usage inside register():
//   .register(|svc| {
//       register_handlers!(svc,
//           HelloRequest => String => HelloHandler,
//           DeleteUserRequest => () => DeleteUserHandler,
//       )
//   })
// =========================================================================
#[deprecated(
    since = "0.2.0",
    note = "HTTP handlers should use `#[handler]` / `#[handler(inject)]` (inventory). \
            register_handlers! remains for non-HTTP Mediator scenarios only."
)]
#[macro_export]
macro_rules! register_handlers {
    ($svc:ident, $($req:ty => $rsp:ty => $handler:ty),+ $(,)?) => {
        $svc
        $(
            .singleton::<dyn $crate::IRequestHandler<$req, $rsp>>(
                |_| ::std::sync::Arc::new(<$handler>::default())
            )
        )+
    };
}
