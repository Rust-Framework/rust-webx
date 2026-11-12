//! Type scanning and automatic service registration logic.
//!
//! In the rust-webx framework, "scanning" is achieved at compile time via:
//!
//! 1. `#[rust_dix::module]` + `rust_dix::inject!` — declare handlers in a module group
//! 2. `#[endpoint]` (or shortcuts `#[get]`, `#[post]`, `#[put]`, `#[delete]`) — register route metadata via `inventory::submit!`
//!
//! This module provides the `RouteEntry` type that connects compile-time
//! macro output to runtime routing.

use crate::localization::TextRef;
use crate::routing::HttpMethod;
use std::any::Any;
use std::collections::HashMap;

/// Metadata about a request parameter for OpenAPI generation.
#[derive(Debug, Clone)]
pub struct ParamMeta {
    /// Field name (e.g., "id", "body").
    pub name: &'static str,

    /// Parameter location: "path", "query", "header", "form", "body",
    /// "claim", or "" for an unmarked field that the route method resolves.
    pub source: &'static str,

    /// Type hint for OpenAPI schema (e.g., "string", "integer", "object").
    pub type_hint: &'static str,

    /// Whether the parameter must be present (`Option<T>` and `Vec<T>` are
    /// not required; path parameters always are).
    pub required: bool,

    /// The JSON object key this parameter binds from, when it can bind from a
    /// JSON body: an explicit `#[frombody]` field, a composite field, or an
    /// unmarked field on a write method (the route method decides; `source` is
    /// `""` for those).
    ///
    /// `None` for parameters bound from the path, query, header, form or
    /// claims, and for a body field with no JSON key of its own — a
    /// `#[serde(flatten)]` field spreads its inner properties into the body
    /// object instead of owning one key.
    pub json_key: Option<&'static str>,
}

/// One `#[authorize]` declaration on an endpoint.
///
/// Every rule on an endpoint must be satisfied (AND). Within a rule, a non-empty
/// `roles` list means "the user has any of these roles" and a non-empty
/// `permissions` list means "the user has any of these permissions"; the two
/// dimensions are ANDed. Empty lists leave a dimension unconstrained, so a rule
/// with both empty means "any authenticated user".
///
/// Multiple values in one declaration
/// (`#[authorize(roles = ["admin", "auditor"])]`) express alternatives; multiple
/// `#[authorize]` attributes express requirements that all have to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct AuthorizeRule {
    /// Any-of role names; empty = no role constraint.
    pub roles: &'static [&'static str],

    /// Any-of permission names; empty = no permission constraint.
    pub permissions: &'static [&'static str],
}

impl AuthorizeRule {
    /// A rule that only requires a valid authenticated principal.
    pub const fn authenticated() -> Self {
        Self {
            roles: &[],
            permissions: &[],
        }
    }

    /// Whether this rule is satisfied by the given claims.
    ///
    /// The `"authenticated"` sentinel is accepted in `roles` for hand-written
    /// metadata; a rule with no constraints has the same meaning.
    pub fn matches(&self, roles: &[String], permissions: &[String]) -> bool {
        let role_ok = self.roles.is_empty()
            || self.roles.iter().any(|required| {
                *required == "authenticated" || roles.iter().any(|actual| actual == required)
            });
        let permission_ok = self.permissions.is_empty()
            || self
                .permissions
                .iter()
                .any(|required| permissions.iter().any(|actual| actual == required));
        role_ok && permission_ok
    }
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

    /// Compile-time identity of the request type this route dispatches to.
    ///
    /// OpenAPI looks the request type's parameter metadata up by this identity,
    /// so a qualified `self_ty` (`contracts::ListOrdersRequest`) and two
    /// same-named request types in different modules/crates stay distinct.
    /// `TypeId::of::<()>()` is the "not provided" sentinel written by
    /// [`RouteEntry::new`]; [`RouteEntry::with_type_id`] fills it in.
    pub req_type_id: std::any::TypeId,

    /// Type name of the request handler.
    /// Used to dispatch to the correct handler at runtime.
    pub handler_type: &'static str,

    /// OpenAPI response type name (e.g., `"UserModel"`, `"Vec<UserModel>"`, `"String"`).
    pub rsp_type: &'static str,

    /// Human-readable endpoint name, as a resource reference.
    ///
    /// From `#[display]`, else the first line of the impl block's `///` doc
    /// comment, else a name generated from the request type.
    pub name: TextRef,

    /// Long-form description, as a resource reference.
    ///
    /// From `#[description]`, else the impl block's `///` doc comment.
    pub description: TextRef,

    /// Optional grouping label, as a resource reference.
    ///
    /// From `#[category]`; empty when the endpoint is uncategorized. Categories
    /// are for display grouping only — they do not affect routing or
    /// authorization.
    pub category: TextRef,

    /// OpenAPI parameter metadata: path params, body params, etc.
    pub params: &'static [ParamMeta],

    /// Authorization rules from `#[authorize]`; empty = public endpoint.
    pub authorization: &'static [AuthorizeRule],
}

// Collect RouteEntry instances at compile time using `inventory`.
inventory::collect!(RouteEntry);

/// Optional query/path/body metadata for a request type, collected via
/// `#[derive(WebxRequestMeta)]` on the request struct.
#[derive(Debug, Clone)]
pub struct RequestParamEntry {
    /// Request type name, kept for diagnostics and for the legacy name lookup.
    ///
    /// The bare name is not unique (qualified paths, same-named types in
    /// different modules), so consumers must prefer [`Self::req_type_id`].
    pub request_type: &'static str,

    /// Compile-time identity of the request type — the authoritative key.
    ///
    /// The derive is the only producer of real values. Hand-written entries can
    /// use `TypeId::of::<()>()` as the "unknown" sentinel and fall back to the
    /// name.
    pub req_type_id: std::any::TypeId,

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

/// The signature every route dispatch function implements.
///
/// It receives the live request context — headers, route parameters, claims and
/// the body stream — and returns the response to write.
pub type RouteDispatchFn = fn(
    ctx: &mut dyn crate::http::IHttpContext,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = crate::error::Result<crate::action::Response>> + Send + '_,
    >,
>;

/// A dispatch function registered at compile time via the endpoint macros.
///
/// The function receives the live request context, so it can inspect headers,
/// stream the body as `multipart/form-data`, and produce any
/// [`Response`](crate::action::Response) — including streamed file downloads.
pub struct RouteDispatch {
    pub handler_type: &'static str,
    pub dispatch: RouteDispatchFn,
}

inventory::collect!(RouteDispatch);

impl RouteEntry {
    /// Create a new route entry without a request type identity.
    ///
    /// Kept for hand-written entries and fixtures; [`RouteEntry::with_type_id`]
    /// records the request type's `TypeId` so OpenAPI can index its parameter
    /// metadata unambiguously.
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        method: HttpMethod,
        path: &'static str,
        handler_type: &'static str,
        rsp_type: &'static str,
        name: TextRef,
        description: TextRef,
        category: TextRef,
        params: &'static [ParamMeta],
        authorization: &'static [AuthorizeRule],
    ) -> Self {
        Self {
            method,
            path,
            req_type_id: std::any::TypeId::of::<()>(),
            handler_type,
            rsp_type,
            name,
            description,
            category,
            params,
            authorization,
        }
    }

    /// Create a new route entry carrying its request type's `TypeId`.
    ///
    /// The endpoint macros use this so OpenAPI resolves parameter metadata by
    /// type instead of by a name string.
    #[allow(clippy::too_many_arguments)]
    pub const fn with_type_id(
        method: HttpMethod,
        path: &'static str,
        req_type_id: std::any::TypeId,
        handler_type: &'static str,
        rsp_type: &'static str,
        name: TextRef,
        description: TextRef,
        category: TextRef,
        params: &'static [ParamMeta],
        authorization: &'static [AuthorizeRule],
    ) -> Self {
        Self {
            method,
            path,
            req_type_id,
            handler_type,
            rsp_type,
            name,
            description,
            category,
            params,
            authorization,
        }
    }
}

// --- Handler Cache ---

/// Boxed future returned by a type-erased handler dispatch.
pub type BoxedDispatchFuture = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = crate::error::Result<Box<dyn std::any::Any + Send>>>
            + Send,
    >,
>;

/// Type-erased dispatch function for one request type.
///
/// `#[handler]` generates one per `impl IRequestHandler<T, R>`: it downcasts the
/// boxed request to `T`, runs the typed dispatch (pipeline + handler) and boxes
/// the `R` response. This is what lets `IMediator::send_boxed` work through
/// `Arc<dyn IMediator>` without knowing `T`/`R` at the call site.
pub type BoxedDispatchFn = fn(
    std::sync::Arc<rust_dix::ServiceProvider>,
    std::sync::Arc<HandlerCache>,
    Box<dyn std::any::Any + Send>,
) -> BoxedDispatchFuture;

/// Inventory item tying a request type to its type-erased dispatch function.
pub struct BoxedDispatch {
    /// The request type's `TypeId`.
    pub req_type_id: std::any::TypeId,
    /// Dispatch for that request type.
    pub dispatch: BoxedDispatchFn,
}

inventory::collect!(BoxedDispatch);

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
    boxed_dispatch: HashMap<std::any::TypeId, BoxedDispatchFn>,
}

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
        let mut boxed_dispatch = HashMap::new();
        for reg in inventory::iter::<BoxedDispatch> {
            boxed_dispatch.insert(reg.req_type_id, reg.dispatch);
        }
        Self {
            entries,
            entries_by_id,
            boxed_dispatch,
        }
    }

    /// Look up a handler entry by request type name (route / OpenAPI diagnostics).
    pub fn get(&self, req_type_name: &str) -> Option<&HandlerEntry> {
        self.entries.get(req_type_name)
    }

    /// Look up a handler entry by request type id (Mediator / in-process dispatch).
    pub fn get_by_type_id(&self, type_id: std::any::TypeId) -> Option<&HandlerEntry> {
        self.entries_by_id.get(&type_id)
    }

    /// Type-erased dispatch for `IMediator::send_boxed`.
    ///
    /// `None` when the request type has no `#[handler]` (or a hand-written
    /// handler registration without the boxed bridge).
    pub fn boxed_dispatch(&self, type_id: std::any::TypeId) -> Option<BoxedDispatchFn> {
        self.boxed_dispatch.get(&type_id).copied()
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

#[cfg(test)]
mod tests {
    /// A response DTO that cannot be represented in JSON — the stand-in for the
    /// server-side defect this mapping exists to catch.
    struct AlwaysFails;

    impl serde::Serialize for AlwaysFails {
        fn serialize<S: serde::Serializer>(
            &self,
            _serializer: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("DTO cannot be serialized"))
        }
    }

    /// A response that discards everything: enough for the serialization path.
    struct DiscardResponse;

    #[async_trait::async_trait]
    impl crate::http::IHttpResponse for DiscardResponse {
        fn status(&self) -> u16 {
            200
        }
        fn set_status(&mut self, _code: u16) {}
        fn set_header(&mut self, _key: &str, _value: &str) {}
        async fn write_bytes(&mut self, _data: Vec<u8>) -> crate::error::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn json_response_serialization_failure_is_a_server_error() {
        let err = crate::http::write_json_response(&mut DiscardResponse, &AlwaysFails)
            .await
            .unwrap_err();
        assert!(
            matches!(err, crate::error::Error::ResponseSerialization(_)),
            "response serialization must not reuse the request-parse variant: {err:?}"
        );
        assert_eq!(err.status_code(), 500);
        assert_eq!(err.public_detail(), None);
    }
}
