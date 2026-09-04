//! Type scanning and automatic service registration logic.
//!
//! In the rust-webx framework, "scanning" is achieved at compile time via:
//!
//! 1. `#[rust_dix::module]` + `rust_dix::inject!` — declare handlers in a module group
//! 2. `#[endpoint]` (or shortcuts `#[get]`, `#[post]`, `#[put]`, `#[delete]`) — register route metadata via `inventory::submit!`
//!
//! This module provides the `RouteEntry` type that connects compile-time
//! macro output to runtime routing.

use crate::routing::HttpMethod;
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

// --- Deprecated process-wide fallback (prefer [`DispatchRuntime`] on `Host`) ---

static GLOBAL_PROVIDER: RwLock<Option<Arc<rust_dix::ServiceProvider>>> = RwLock::new(None);

/// Deprecated shim: set a process-wide `ServiceProvider` fallback.
///
/// `Host::build()` no longer calls this. Prefer `Host::provider()` or run work
/// inside `Host::dispatch_runtime().run()` so `dispatch_provider()` resolves
/// from the active runtime.
#[deprecated(
    since = "0.2.0",
    note = "Use Host::provider() or dispatch_provider() within an active DispatchRuntime"
)]
pub fn set_global_provider(provider: Arc<rust_dix::ServiceProvider>) {
    let mut guard = GLOBAL_PROVIDER
        .write()
        .expect("global provider lock poisoned");
    if guard.is_some() {
        tracing::warn!(
            "Replacing deprecated global ServiceProvider. \
             Prefer Host::dispatch_runtime() for per-host isolation."
        );
    }
    *guard = Some(provider);
}

/// Deprecated shim: read the process-wide `ServiceProvider` fallback.
///
/// Panics when no shim was set and no [`DispatchRuntime`] is active on this task.
#[deprecated(
    since = "0.2.0",
    note = "Use Host::provider() or dispatch_provider() within an active DispatchRuntime"
)]
pub fn global_provider() -> Arc<rust_dix::ServiceProvider> {
    try_global_provider().unwrap_or_else(|| {
        panic!(
            "global_provider() is deprecated and not set. \
             Use host.provider() for the host instance, or dispatch_provider() \
             inside Host::dispatch_runtime().run() (HTTP and hosted services are scoped automatically)."
        )
    })
}

/// Internal fallback for `dispatch_provider()` when no task-local runtime is active.
pub(crate) fn try_global_provider() -> Option<Arc<rust_dix::ServiceProvider>> {
    GLOBAL_PROVIDER
        .read()
        .expect("global provider lock poisoned")
        .clone()
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

/// A route entry registered at compile time via `#[endpoint]` (or its shortcuts `#[get]`/`#[post]`/...).
///
/// Collected by the `inventory` crate and read at application startup.
#[derive(Debug, Clone)]
pub struct RouteEntry {
    /// HTTP method for this route.
    pub method: HttpMethod,

    /// Route path pattern (e.g., "/users/{id}").
    pub path: &'static str,

    /// Type name of the request handler.
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

    /// "" = public, "authenticated" = any valid JWT, otherwise specific role name.
    pub required_role: &'static str,

    /// Non-empty when `#[authorize(permission = "...")]` is declared.
    pub required_permission: &'static str,
}

// Collect RouteEntry instances at compile time using `inventory`.
inventory::collect!(RouteEntry);

/// Optional query/path/body metadata for a request type, collected via
/// `#[derive(WebxRequestMeta)]` on the request struct.
#[derive(Debug, Clone)]
pub struct RequestParamEntry {
    pub request_type: &'static str,
    pub params: &'static [ParamMeta],
}

inventory::collect!(RequestParamEntry);

/// Handler registration collected at compile time.
/// Each `#[handler]` annotation submits one of these to inventory.
///
/// The factory is called **per request** with the request-scoped `IServiceResolver`
/// so it can resolve Scoped dependencies (e.g. `DbContext`) via `get_owned`.
/// The result is an owned, type-erased handler — no `Arc<dyn Trait>` caching,
/// so `handle(&mut self)` works without interior mutability.
pub struct HandlerRegistration {
    /// Compile-time type id of the request type — primary dispatch key.
    pub req_type_id: std::any::TypeId,
    /// Request type name (e.g., "HelloRequest") — used for diagnostics and route matching.
    pub req_type_name: &'static str,
    /// Constructs a fresh owned handler, boxed as `Box<dyn Any + Send>`.
    /// Called per-request with the per-request scope as resolver.
    pub factory:
        fn(&dyn rust_dix::IServiceResolver) -> crate::error::Result<Box<dyn std::any::Any + Send>>,
    /// Type-erased call bridge: receives the owned handler and the boxed request,
    /// invokes `handle(&mut self, req)`, and returns the boxed response.
    #[allow(clippy::type_complexity)]
    pub call: fn(
        handler: Box<dyn std::any::Any + Send>,
        request: Box<dyn std::any::Any + Send>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = crate::error::Result<Box<dyn std::any::Any + Send>>>
                + Send,
        >,
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

impl RouteEntry {
    /// Create a new route entry.
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        method: HttpMethod,
        path: &'static str,
        handler_type: &'static str,
        rsp_type: &'static str,
        summary: &'static str,
        description: &'static str,
        params: &'static [ParamMeta],
        required_role: &'static str,
        required_permission: &'static str,
    ) -> Self {
        Self {
            method,
            path,
            handler_type,
            rsp_type,
            summary,
            description,
            params,
            required_role,
            required_permission,
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
    pub entries_by_id: HashMap<std::any::TypeId, HandlerEntry>,
}

/// Deprecated process-wide handler registry fallback (inventory is process-wide).
static HANDLER_CACHE: OnceLock<Arc<HandlerCache>> = OnceLock::new();

impl HandlerCache {
    /// Build the cache from all `HandlerRegistration` inventory items.
    pub fn build() -> Self {
        let mut entries = HashMap::new();
        let mut entries_by_id = HashMap::new();
        for reg in inventory::iter::<HandlerRegistration> {
            let entry = HandlerEntry {
                factory: reg.factory,
                call: reg.call,
            };
            entries.insert(reg.req_type_name, entry.clone());
            entries_by_id.insert(reg.req_type_id, entry);
        }
        Self {
            entries,
            entries_by_id,
        }
    }

    /// Deprecated shim: lazy-build the process-wide handler registry.
    ///
    /// `Host::build()` uses `HandlerCache::build()` per instance. This remains only
    /// as a fallback for in-process `Mediator::send` without an active runtime.
    #[deprecated(
        since = "0.2.0",
        note = "Prefer dispatch_handler_cache() within an active DispatchRuntime"
    )]
    pub fn get_or_init() -> Arc<Self> {
        Arc::clone(HANDLER_CACHE.get_or_init(|| Arc::new(Self::build())))
    }

    /// Look up a handler entry by request type name (route / OpenAPI diagnostics).
    pub fn get(&self, req_type_name: &str) -> Option<&HandlerEntry> {
        self.entries.get(req_type_name)
    }

    /// Look up a handler entry by request type id (Mediator / in-process dispatch).
    pub fn get_by_type_id(&self, type_id: std::any::TypeId) -> Option<&HandlerEntry> {
        self.entries_by_id.get(&type_id)
    }
}

/// Preferred name for [`HandlerCache`] — emphasizes registry semantics over caching.
pub type HandlerRegistry = HandlerCache;

/// A single compiled handler entry in the registry.
///
/// Stores only the factory and call bridge — no cached instance.
/// The factory is called per request to produce a fresh owned handler.
#[derive(Clone)]
pub struct HandlerEntry {
    /// Per-request factory: receives the scoped resolver, returns an owned handler.
    pub factory: fn(&dyn rust_dix::IServiceResolver) -> crate::error::Result<Box<dyn Any + Send>>,
    /// Type-erased call bridge: invokes `handle(&mut self, req)` on the owned handler.
    #[allow(clippy::type_complexity)]
    pub call: fn(
        handler: Box<dyn Any + Send>,
        request: Box<dyn Any + Send>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = crate::error::Result<Box<dyn Any + Send>>> + Send>,
    >,
}
