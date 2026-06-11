// lrwf — Umbrella crate for the LRWF framework.
// Re-exports all types for a unified `use lrwf::*` experience.

// --- Core traits ---
pub use lrwf_core::app::{IApplicationBuilder, IHost};
pub use lrwf_core::auth::{
    IAuthenticationHandler, IAuthorizationPolicy, IClaims, IDynamicAuthorizer,
};
pub use lrwf_core::cache::{
    cache_ext::DistributedCacheExtensions,
    options::DistributedCacheEntryOptions,
    trait_def::{CacheError, IDistributedCache},
};
pub use lrwf_core::config::{
    bind_config, bind_root, load_appsettings, AppOptions, AppSection, CorsSection, IAppOptions,
    JwtSection, TlsSection,
};
pub use lrwf_core::error::{Error, Result};
pub use lrwf_core::handler::{IEventHandler, IRequestHandler};
pub use lrwf_core::http::{
    read_json_body, write_json_response, FromHttpContext, HttpStatus, IClaimsExt, IHttpContext,
    IHttpRequest, IHttpResponse, Json,
};
pub use lrwf_core::mediator::{IEventRequest, IMediator, IRequest};
pub use lrwf_core::middleware::IMiddleware;
pub use lrwf_core::mode::AppMode;
pub use lrwf_core::pagination::{PagedRequest, PagedResponse};
pub use lrwf_core::pipeline::{IPipelineBehavior, IServiceResolver};
pub use lrwf_core::problem::{FieldError, ProblemDetails};
pub use lrwf_core::routing::{HttpMethod, IEndpoint, IRouter, RouteMeta};

// --- DI extensions ---
pub use lrwf_core::di::ext::{is_mediator_active, should_scan_endpoints, IServiceCollectionExt};
pub use lrwf_core::di::scan::{
    global_provider, set_global_provider, HandlerCache, HandlerEntry, HandlerRegistration,
    ParamMeta, ResponseData, RouteDispatch, RouteEntry, RouteSource,
};

// --- HTTP layer ---
pub use lrwf_http::auth_jwt::{init_jwt_secret, jwt_middleware, jwt_secret, JwtAuth, JwtClaims};
pub use lrwf_http::authz::{
    collect_authorizers, resource_auth_middleware, AuthorizerSet, ResourceAuthorization,
};
pub use lrwf_http::compression::{compress_gzip, CompressionConfig};
pub use lrwf_http::context::{HttpContext, HttpRequest, HttpResponse};
pub use lrwf_http::cors::{CorsConfig, CorsMiddleware};
pub use lrwf_http::endpoint::{
    ControllerEndpoint, RequestEndpoint, StaticHtmlEndpoint, StaticJsonEndpoint,
};
pub use lrwf_http::health::{HealthCheckFn, HealthCheckRegistry, HealthStatus};
pub use lrwf_http::memory_cache::MemoryCache;
pub use lrwf_http::pipeline::{HandlerFn, MiddlewarePipeline};
pub use lrwf_http::rate_limit::{RateLimitMiddleware, RateLimiter};
pub use lrwf_http::request_id::RequestIdMiddleware;
pub use lrwf_http::request_tracing::RequestTracing;
pub use lrwf_http::router::Router;
pub use lrwf_http::security_headers::SecurityHeadersMiddleware;
pub use lrwf_http::server::{Host, HostAppBuilder, HostBuilder, Server, ServerHandle};
pub use lrwf_http::timing::TimingMiddleware;

// --- Mediator ---
pub use lrwf_core::mediator_impl::Mediator;

// --- Web (SPA) ---
pub use lrwf_web::SpaMiddleware;

// --- OpenAPI ---
pub use lrwf_openapi::{generate_openapi_spec, APIUI_HTML};

// --- Macros ---
pub use lrwf_macros::{
    authorize, controller, delete, endpoint, from_body, from_query, from_route, get, handler,
    http_delete, http_get, http_post, http_put, post, put, request,
};

// --- Re-export lrdi (for manual registration, #[inject] auto-registration, and module blocks) ---
pub use lrdi;
pub use lrdi::inject;
pub use lrdi::inject_attr;
pub use lrdi_macros;
pub use lrdi_macros::{module, Inject};

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
