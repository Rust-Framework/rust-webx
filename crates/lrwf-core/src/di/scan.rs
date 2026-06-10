//! Type scanning and automatic service registration logic.
//!
//! In the LRWF framework, "scanning" is achieved at compile time via:
//!
//! 1. `#[lrdi::module]` + `lrdi::inject!` — declare handlers in a module group
//! 2. `#[endpoint]` — register route metadata via `inventory::submit!`
//! 3. `#[controller]` — register controller metadata via `inventory::submit!`
//!
//! This module provides the `RouteEntry` type that connects compile-time
//! macro output to runtime routing.

use crate::routing::HttpMethod;

/// Metadata about a request parameter for OpenAPI generation.
#[derive(Debug, Clone)]
pub struct ParamMeta {
    /// Field name (e.g., "id", "body").
    pub name: &'static str,

    /// Parameter location: "path", "query", or "body".
    pub source: &'static str,

    /// Type hint for OpenAPI schema (e.g., "string", "integer", "object").
    pub type_hint: &'static str,
}

/// A route entry registered at compile time via `#[endpoint]` or `#[controller]`.
///
/// Collected by the `inventory` crate and read at application startup.
#[derive(Debug, Clone)]
pub struct RouteEntry {
    /// HTTP method for this route.
    pub method: HttpMethod,

    /// Route path pattern (e.g., "/users/{id}").
    pub path: &'static str,

    /// Type name of the request or controller handler.
    /// Used to dispatch to the correct handler at runtime.
    pub handler_type: &'static str,

    /// OpenAPI response type name (e.g., "UserModel", "Vec<UserModel>", "String").
    pub rsp_type: &'static str,

    /// Human-readable summary for OpenAPI docs (e.g., "Get user by ID").
    pub summary: &'static str,

    /// Optional long-form description from `///` doc comments on the impl block.
    pub description: &'static str,

    /// OpenAPI parameter metadata: path params, body params, etc.
    pub params: &'static [ParamMeta],

    /// Source kind: "request" for IRequest endpoints, "controller" for controller methods.
    pub source: RouteSource,

    /// "" = public, "authenticated" = any valid JWT, otherwise specific role name.
    pub required_role: &'static str,
}

/// Distinguishes between IRequest-based and Controller-based endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSource {
    /// Endpoint registered via `#[endpoint]` on an `impl IRequest` block.
    RequestEndpoint,

    /// Endpoint registered via `#[controller]` with method attributes.
    ControllerMethod,
}

// Collect RouteEntry instances at compile time using `inventory`.
inventory::collect!(RouteEntry);

/// Handler registration collected at compile time.
/// Each `#[handler]` annotation submits one of these to inventory.
pub struct HandlerRegistration {
    /// Function that registers this handler into a ServiceCollection.
    /// Takes ownership, calls `.singleton(...)`, and returns the updated collection.
    pub register: fn(svc: lrdi::ServiceCollection) -> lrdi::ServiceCollection,
}

inventory::collect!(HandlerRegistration);

/// A dispatch function registered at compile time via the endpoint macros.
///
/// Each `#[get]`, `#[post]`, etc. macro generates one of these with a
/// function that constructs the request from `IHttpContext`, calls the
/// mediator, and writes the JSON response.
pub struct RouteDispatch {
    /// The handler type name (used to match RouteEntry).
    pub handler_type: &'static str,
    /// Boxed async dispatch function.
    /// Takes request data and writes JSON response via the mediator.
    #[allow(clippy::type_complexity)]
    pub dispatch: fn(
        body_bytes: Vec<u8>,
        route_params: std::collections::HashMap<String, String>,
        query_params: std::collections::HashMap<String, String>,
        provider: std::sync::Arc<lrdi::ServiceProvider>,
        claims: Option<Box<dyn crate::auth::IClaims>>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = crate::error::Result<ResponseData>> + Send>,
    >,
}

/// Response data produced by a dispatch function.
#[derive(Debug)]
pub struct ResponseData {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

inventory::collect!(RouteDispatch);

// SAFETY: RouteDispatch is only used at startup, single-threaded context.
unsafe impl Sync for RouteDispatch {}
unsafe impl Send for RouteDispatch {}

impl RouteEntry {
    /// Create a new request-based route entry.
    pub const fn request(
        method: HttpMethod,
        path: &'static str,
        handler_type: &'static str,
        rsp_type: &'static str,
        summary: &'static str,
        description: &'static str,
        params: &'static [ParamMeta],
        required_role: &'static str,
    ) -> Self {
        Self {
            method,
            path,
            handler_type,
            rsp_type,
            summary,
            description,
            params,
            source: RouteSource::RequestEndpoint,
            required_role,
        }
    }

    /// Create a new controller-based route entry.
    pub const fn controller(
        method: HttpMethod,
        path: &'static str,
        handler_type: &'static str,
        rsp_type: &'static str,
        summary: &'static str,
        description: &'static str,
        params: &'static [ParamMeta],
        required_role: &'static str,
    ) -> Self {
        Self {
            method,
            path,
            handler_type,
            rsp_type,
            summary,
            description,
            params,
            source: RouteSource::ControllerMethod,
            required_role,
        }
    }
}
