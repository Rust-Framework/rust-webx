// rust-webapp â€?Umbrella crate for the Rust WebApi framework.
// Re-exports all types for a unified `use rust_webapp::*` experience.

// --- Core traits ---
pub use rust_webapp_core::app::{IApplicationBuilder, IHost};
pub use rust_webapp_core::auth::{
    IAuthenticationHandler, IAuthorizationPolicy, IClaims, IDynamicAuthorizer,
};
pub use rust_webapp_core::cache::{
    cache_ext::DistributedCacheExtensions,
    options::DistributedCacheEntryOptions,
    trait_def::{CacheError, IDistributedCache},
};
pub use rust_webapp_core::config::{
    bind_config, bind_root, load_appsettings, AppOptions, AppSection, CorsSection, IAppOptions,
    JwtSection, TlsSection,
};
pub use rust_webapp_core::error::{Error, Result};
pub use rust_webapp_core::handler::{IEventHandler, IHostedService, IRequestHandler};
pub use rust_webapp_core::http::{
    read_json_body, write_json_response, FromHttpContext, HttpStatus, IClaimsExt, IHttpContext,
    IHttpRequest, IHttpResponse, Json,
};
pub use rust_webapp_core::mediator::{IEventRequest, IMediator, IRequest};
pub use rust_webapp_core::middleware::IMiddleware;
pub use rust_webapp_core::mode::AppMode;
pub use rust_webapp_core::pagination::{PagedRequest, PagedResponse};
pub use rust_webapp_core::paths::{app_base, looks_like_app_base};
pub use rust_webapp_core::pipeline::{IPipelineBehavior, IServiceResolver};
pub use rust_webapp_core::problem::{FieldError, ProblemDetails};
pub use rust_webapp_core::routing::{HttpMethod, IEndpoint, IRouter, RouteMeta};

// --- DI extensions ---
pub use rust_webapp_core::di::ext::{is_mediator_active, should_scan_endpoints, IServiceCollectionExt};
pub use rust_webapp_core::di::scan::{
    global_provider, set_global_provider, HandlerCache, HandlerEntry, HandlerRegistration,
    ParamMeta, ResponseData, RouteDispatch, RouteEntry, RouteSource,
};

// --- HTTP layer ---
pub use rust_webapp_host::auth_jwt::{init_jwt_secret, jwt_middleware, jwt_secret, JwtAuth, JwtClaims};
pub use rust_webapp_host::authz::{
    collect_authorizers, resource_auth_middleware, AuthorizerSet, ResourceAuthorization,
};
pub use rust_webapp_host::compression::{compress_gzip, CompressionConfig};
pub use rust_webapp_host::context::{HttpContext, HttpRequest, HttpResponse};
pub use rust_webapp_host::cors::{CorsConfig, CorsMiddleware};
pub use rust_webapp_host::endpoint::{
    ControllerEndpoint, RequestEndpoint, StaticHtmlEndpoint, StaticJsonEndpoint,
};
pub use rust_webapp_host::health::{HealthCheckFn, HealthCheckRegistry, HealthStatus};
pub use rust_webapp_host::memory_cache::MemoryCache;
pub use rust_webapp_host::pipeline::{HandlerFn, MiddlewarePipeline};
pub use rust_webapp_host::rate_limit::{RateLimitMiddleware, RateLimiter};
pub use rust_webapp_host::request_id::RequestIdMiddleware;
pub use rust_webapp_host::request_tracing::RequestTracing;
pub use rust_webapp_host::router::Router;
pub use rust_webapp_host::security_headers::SecurityHeadersMiddleware;
pub use rust_webapp_host::server::{Host, HostAppBuilder, HostBuilder, Server, ServerHandle};
pub use rust_webapp_host::timing::TimingMiddleware;

// --- Mediator ---
pub use rust_webapp_core::mediator_impl::Mediator;

// --- Web (SPA) ---
pub use rust_webapp_spa::SpaMiddleware;

// --- OpenAPI ---
pub use rust_webapp_openapi::{generate_openapi_spec, APIUI_HTML};

// --- Macros ---
pub use rust_webapp_macros::{
    authorize, controller, delete, endpoint, from_body, from_query, from_route, get, handler,
    http_delete, http_get, http_post, http_put, post, put, request,
};

// --- Re-export rust_dicore (for manual registration, #[inject] auto-registration, and module blocks) ---
pub use rust_dicore;
pub use rust_dicore::inject;
pub use rust_dicore::inject_attr;
pub use rust_dicore_macros;
pub use rust_dicore_macros::{module, Inject};

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
