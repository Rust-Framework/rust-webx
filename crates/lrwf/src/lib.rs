// lrwf — Umbrella crate for the LRWF framework.
// Re-exports all types for a unified `use lrwf::*` experience.

// --- Core traits ---
pub use lrwf_core::app::{IApplicationBuilder, IHost};
pub use lrwf_core::auth::{IAuthenticationHandler, IAuthorizationPolicy, IClaims};
pub use lrwf_core::config::{bind_config, bind_root, load_appsettings, AppOptions, AppSection, CorsSection, JwtSection, IAppOptions};
pub use lrwf_core::error::{Error, Result};
pub use lrwf_core::handler::{IEventHandler, IRequestHandler};
pub use lrwf_core::http::{
    read_json_body, write_json_response, HttpStatus, IClaimsExt, IHttpContext, IHttpRequest,
    IHttpResponse, Json,
};
pub use lrwf_core::mediator::{IEventRequest, IMediator, IRequest};
pub use lrwf_core::middleware::IMiddleware;
pub use lrwf_core::mode::AppMode;
pub use lrwf_core::pipeline::{IPipelineBehavior, IServiceResolver};
pub use lrwf_core::routing::{HttpMethod, IEndpoint, IRouter, RouteMeta};

// --- DI extensions ---
pub use lrwf_core::di::ext::{should_scan_endpoints, is_mediator_active, IServiceCollectionExt};
pub use lrwf_core::di::scan::{HandlerRegistration, ParamMeta, RouteEntry, RouteSource};

// --- HTTP layer ---
pub use lrwf_http::auth_jwt::{JwtAuth, JwtClaims, jwt_middleware};
pub use lrwf_http::authz::{ResourceAuthorization, resource_auth_middleware};
pub use lrwf_http::context::{HttpContext, HttpRequest, HttpResponse};
pub use lrwf_http::cors::{CorsConfig, CorsMiddleware};
pub use lrwf_http::endpoint::{
    ControllerEndpoint, RequestEndpoint, StaticHtmlEndpoint, StaticJsonEndpoint,
};
pub use lrwf_http::pipeline::{HandlerFn, MiddlewarePipeline};
pub use lrwf_http::router::Router;
pub use lrwf_http::server::{Host, HostAppBuilder, HostBuilder};

// --- Mediator ---
pub use lrwf_core::mediator_impl::Mediator;

// --- Web (SPA) ---
pub use lrwf_web::SpaMiddleware;

// --- OpenAPI ---
pub use lrwf_openapi::{generate_openapi_spec, APIUI_HTML};

// --- Macros ---
pub use lrwf_macros::{
    controller, endpoint,
    get, post, put, delete,
    from_body, from_query, from_route,
    handler,
    http_delete, http_get, http_post, http_put,
    request,
};

// --- Re-export lrdi (for manual registration and module blocks) ---
pub use lrdi;
pub use lrdi_macros;
pub use lrdi_macros::module;
pub use lrdi::inject;

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
