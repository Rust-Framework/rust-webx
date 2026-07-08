// rust-webx —Umbrella crate for the Rust WebApi framework.
// Re-exports all types for a unified `use rust_webx::*` experience.

// --- Core traits ---
pub use rust_webx_core::app::IHost;
pub use rust_webx_core::auth::{
    IAuthenticationHandler, IAuthorizationPolicy, IClaims, IDynamicAuthorizer,
};
pub use rust_webx_core::cache::{
    cache_ext::DistributedCacheExtensions,
    options::DistributedCacheEntryOptions,
    trait_def::{CacheError, IDistributedCache},
};
pub use rust_webx_core::config::{
    bind_config, bind_root, load_appsettings, AppOptions, AppSection, CorsSection, IAppOptions,
    JwtSection, MetricsSection, RateLimitSection, TlsSection,
};
pub use rust_webx_core::error::{Error, Result};
pub use rust_webx_core::handler::{IClaimsCarrier, IEventHandler, IHostedService, IRequestHandler};
pub use rust_webx_core::http::{
    read_json_body, write_json_response, FromHttpContext, HttpStatus, IClaimsExt, IHttpContext,
    IHttpRequest, IHttpResponse, Json,
};
pub use rust_webx_core::mediator::{IEventRequest, IMediator, IRequest};
pub use rust_webx_core::middleware::IMiddleware;
pub use rust_webx_core::mode::AppMode;
pub use rust_webx_core::pagination::{PagedRequest, PagedResponse};
pub use rust_webx_core::paths::{app_base, looks_like_app_base};
pub use rust_webx_core::mediator::build_pipeline_chain;
pub use rust_webx_core::pipeline::{BoxedNextFn, BoxedPipelineFuture, IPipelineBehavior};
pub use rust_webx_core::problem::{FieldError, ProblemDetails};
pub use rust_webx_core::route::diagnostics::{
    format_route_diagnostics, orphan_handlers, orphan_routes, route_snapshots,
};
pub use rust_webx_core::request_context::RequestContext;
pub use rust_webx_core::routing::{HttpMethod, IEndpoint, IRouter, RouteMeta};

// --- DI extensions ---
pub use rust_webx_core::route::ext::{is_mediator_active, should_scan_endpoints, IServiceCollectionExt};
pub use rust_webx_core::route::scan::{
    global_provider, set_global_provider, HandlerCache, HandlerEntry, HandlerRegistration,
    ParamMeta, ResponseData, RouteDispatch, RouteEntry,
};

// --- HTTP layer ---
pub use rust_webx_host::auth_jwt::{init_jwt_secret, jwt_middleware, jwt_secret, JwtAuth, JwtClaims};
pub use rust_webx_host::authz::{
    collect_authorizers, resource_auth_middleware, AuthorizerSet, ResourceAuthorization,
};
pub use rust_webx_host::compression::{compress_gzip, CompressionConfig, CompressionMiddleware};
pub use rust_webx_host::context::{HttpContext, HttpRequest, HttpResponse};
pub use rust_webx_host::cors::{CorsConfig, CorsMiddleware};
pub use rust_webx_host::endpoint::{
    ControllerEndpoint, RequestEndpoint, StaticHtmlEndpoint, StaticJsonEndpoint,
};
pub use rust_webx_host::health::{HealthCheckEntry, HealthCheckFn, HealthCheckRegistry, HealthStatus};
pub use rust_webx_host::memory_cache::MemoryCache;
pub use rust_webx_host::pipeline::{HandlerFn, MiddlewarePipeline};
pub use rust_webx_host::rate_limit::{RateLimitMiddleware, RateLimiter};
pub use rust_webx_host::request_id::RequestIdMiddleware;
pub use rust_webx_host::request_tracing::RequestTracing;
pub use rust_webx_host::router::Router;
pub use rust_webx_host::security_headers::SecurityHeadersMiddleware;
pub use rust_webx_host::diagnostics::log_startup_diagnostics;
pub use rust_webx_host::server::{Host, HostAppBuilder, HostBuilder, Server, ServerHandle};
pub use rust_webx_host::timing::TimingMiddleware;
#[cfg(feature = "testing")]
pub use rust_webx_host::testing::{free_port, spawn, spawn_with_health, wait_until_ready, TestServer};

// --- Mediator ---
pub use rust_webx_core::mediator::Mediator;

// --- Web (SPA) ---
pub use rust_webx_spa::SpaMiddleware;

// --- OpenAPI ---
pub use rust_webx_openapi::{generate_openapi_spec, APIUI_HTML};

// --- Macros ---
pub use rust_webx_macros::{
    authorize, claims, delete, endpoint, from_body, from_query, from_route,
    get, handler, post, put,
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
// Generates chained .singleton() calls for handlers that implement Default.
//
// Usage inside register():
//   .register(|svc| {
//       register_handlers!(svc,
//           HelloRequest => String => HelloHandler,
//           DeleteUserRequest => () => DeleteUserHandler,
//       )
//   })
//
// Expands to:
//   svc
//     .singleton::<dyn IRequestHandler<HelloRequest, String>>(|_| Arc::new(HelloHandler::default()))
//     .singleton::<dyn IRequestHandler<DeleteUserRequest, ()>>(|_| Arc::new(DeleteUserHandler::default()))
// =========================================================================
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
