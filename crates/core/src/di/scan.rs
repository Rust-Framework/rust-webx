//! Type scanning and automatic service registration logic.
//!
//! In the LRWF framework, "scanning" is achieved at compile time via:
//!
//! 1. `#[rust_dicore::module]` + `rust_dicore::inject!` — declare handlers in a module group
//! 2. `#[endpoint]` — register route metadata via `inventory::submit!`
//! 3. `#[controller]` — register controller metadata via `inventory::submit!`
//!
//! This module provides the `RouteEntry` type that connects compile-time
//! macro output to runtime routing.

use crate::routing::HttpMethod;
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

// --- Global Service Provider (for DI-based handler construction) ---

static GLOBAL_PROVIDER: OnceLock<Arc<rust_dicore::ServiceProvider>> = OnceLock::new();

/// Set the global service provider. Called once at `Host::build()` time.
pub fn set_global_provider(provider: Arc<rust_dicore::ServiceProvider>) {
    GLOBAL_PROVIDER.set(provider).ok();
}

/// Get the global service provider. Used by `#[handler]` factories
/// when the handler struct has `#[inject_attr]` for DI-based construction.
pub fn global_provider() -> &'static Arc<rust_dicore::ServiceProvider> {
    GLOBAL_PROVIDER
        .get()
        .expect("Global provider not initialized")
}

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
///
/// The factory is called **per request** with the request-scoped `IServiceResolver`
/// so it can resolve Scoped dependencies (e.g. `DbContext`) via `get_owned`.
/// The result is an owned, type-erased handler — no `Arc<dyn Trait>` caching,
/// so `handle(&mut self)` works without interior mutability.
pub struct HandlerRegistration {
    /// Request type name (e.g., "hello_request::HelloRequest") — used to match RouteDispatch.
    pub req_type_name: &'static str,
    /// Constructs a fresh owned handler, boxed as `Box<dyn Any + Send>`.
    /// Called per-request with the per-request scope as resolver.
    pub factory: fn(&dyn rust_dicore::IServiceResolver) -> Box<dyn std::any::Any + Send>,
    /// Type-erased call bridge: receives the owned handler and the boxed request,
    /// invokes `handle(&mut self, req)`, and returns the boxed response.
    #[allow(clippy::type_complexity)]
    pub call: fn(
        handler: Box<dyn std::any::Any + Send>,
        request: Box<dyn std::any::Any + Send>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = crate::error::Result<Box<dyn std::any::Any + Send>>> + Send>,
    >,
}

inventory::collect!(HandlerRegistration);

/// A dispatch function registered at compile time via the endpoint macros.
///
/// Each `#[get]`, `#[post]`, etc. macro generates one of these with a
/// function that constructs the request, looks up the handler, and
/// calls through the HandlerCache call bridge.
pub struct RouteDispatch {
    pub handler_type: &'static str,
    #[allow(clippy::type_complexity)]
    pub dispatch: fn(
        body_bytes: Vec<u8>,
        route_params: std::collections::HashMap<String, String>,
        query_params: std::collections::HashMap<String, String>,
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
    #[allow(clippy::too_many_arguments)]
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
    #[allow(clippy::too_many_arguments)]
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

// --- Handler Cache ---

/// Runtime registry of compiled handler registrations.
///
/// Built at startup from `HandlerRegistration` inventory items.
/// Maps request type name to handler entry (factory + call bridge).
///
/// Unlike the previous design, entries do **not** cache a handler instance:
/// the factory is invoked per request with the request-scoped resolver so
/// Scoped dependencies (DbContext) are freshly owned each time.
pub struct HandlerCache {
    pub entries: HashMap<&'static str, HandlerEntry>,
}

static HANDLER_CACHE: OnceLock<HandlerCache> = OnceLock::new();

impl HandlerCache {
    /// Build the cache from all `HandlerRegistration` inventory items.
    pub fn build() -> Self {
        let mut entries = HashMap::new();
        for reg in inventory::iter::<HandlerRegistration> {
            entries.insert(
                reg.req_type_name,
                HandlerEntry {
                    factory: reg.factory,
                    call: reg.call,
                },
            );
        }
        Self { entries }
    }

    /// Initialize the global cache. Called once at host build time.
    pub fn init_global() {
        let cache = Self::build();
        HANDLER_CACHE.set(cache).ok();
    }

    /// Get a reference to the global handler cache.
    /// Must be called after `init_global()`.
    pub fn get_or_init() -> &'static HandlerCache {
        HANDLER_CACHE.get_or_init(Self::build)
    }

    /// Look up a handler entry by request type name.
    pub fn get(&self, req_type_name: &str) -> Option<&HandlerEntry> {
        self.entries.get(req_type_name)
    }
}

/// A single compiled handler entry in the registry.
///
/// Stores only the factory and call bridge — no cached instance.
/// The factory is called per request to produce a fresh owned handler.
pub struct HandlerEntry {
    /// Per-request factory: receives the scoped resolver, returns an owned handler.
    pub factory: fn(&dyn rust_dicore::IServiceResolver) -> Box<dyn Any + Send>,
    /// Type-erased call bridge: invokes `handle(&mut self, req)` on the owned handler.
    #[allow(clippy::type_complexity)]
    pub call: fn(
        handler: Box<dyn Any + Send>,
        request: Box<dyn Any + Send>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = crate::error::Result<Box<dyn Any + Send>>> + Send>,
    >,
}
