//! The endpoint catalog — the runtime view of every route declared by the route
//! macros.
//!
//! It is built once by `Host::build()` from the compile-time `inventory` table
//! and exposed through the host and its DI container, like ASP.NET Core's
//! `EndpointDataSource`. Application code (permission sync, admin tooling,
//! handlers) resolves the [`IEndpointCatalog`] interface instead of reading
//! `inventory` directly, so it sees the metadata of *this* host rather than a
//! process-wide global.
//!
//! ```ignore
//! // Injected into a hosted service or handler:
//! let catalog: Arc<dyn IEndpointCatalog> = ...;
//! for endpoint in catalog.all() {
//!     println!("{} {} — {}", endpoint.method, endpoint.path, endpoint.name.text);
//! }
//!
//! // Localized, serializable view for an RBAC system:
//! for endpoint in catalog.localized(&*localizer) {
//!     println!("{} {} — {}", endpoint.method, endpoint.path, endpoint.name);
//! }
//! ```
//!
//! The catalog adds no metadata of its own: every field is copied from
//! [`RouteEntry`]. Names and descriptions are resource references; the literal
//! `text` is only a fallback. The active language comes from the host's
//! `Culture.Default` configuration.

use crate::error::Error;
use crate::localization::{IStringLocalizer, TextRef};
use crate::route::scan::{AuthorizeRule, RouteEntry};
use serde::Serialize;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::fmt;

/// Deterministic GUID for one endpoint (RFC 4122 version 5, SHA-1 based).
///
/// Derived from the module-qualified request identity written by the endpoint
/// macro, so it is stable across builds and route-pattern changes. Use it as the
/// primary key of a permission table; the human-readable
/// [`EndpointDescriptor::id`] stays available for debugging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EndpointGuid([u8; 16]);

impl EndpointGuid {
    /// Fixed namespace for all rust-webx endpoint GUIDs (16 bytes, ASCII).
    const NAMESPACE: [u8; 16] = *b"webx-endpoint-v1";

    /// Derive the GUID from the stable request identity.
    pub fn from_request_id(request_id: &str) -> Self {
        let mut hasher = Sha1::new();
        hasher.update(Self::NAMESPACE);
        hasher.update(request_id.as_bytes());
        let digest = hasher.finalize();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        // RFC 4122: version 5 in the high nibble of byte 6, variant 10 in byte 8.
        bytes[6] = (bytes[6] & 0x0f) | 0x50;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Self(bytes)
    }

    /// Raw bytes (for binary columns).
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Build from raw bytes (e.g. a GUID read from the database).
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

impl fmt::Display for EndpointGuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        write!(
            f,
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13],
            b[14], b[15]
        )
    }
}

impl Serialize for EndpointGuid {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// One endpoint's language-neutral metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EndpointDescriptor {
    /// Stable endpoint identifier: the qualified request type name
    /// (`docbit_contracts::blog::ListBlogPostsRequest`).
    ///
    /// Unlike `path`, it survives route-pattern changes and is unique per
    /// endpoint in the process — [`EndpointCatalog::try_from_inventory`] fails
    /// (`Error::di`) instead of returning an ambiguous catalog when two routes
    /// declare the same `id`. Use it as a readable key; prefer [`Self::guid`]
    /// as the external permission primary key. `(method, path)` remains the
    /// natural key the runtime authorization uses.
    pub id: &'static str,

    /// Deterministic GUID derived from [`Self::id`] (RFC 4122 v5): the preferred
    /// primary key for a permission table (compact, opaque, stable).
    pub guid: EndpointGuid,

    /// Canonical upper-case HTTP method (`"GET"`, `"POST"`, …).
    pub method: &'static str,

    /// The route pattern exactly as declared in the macro, e.g.
    /// `/api/blog/{slug}`. This is the original string — not normalized and not
    /// the concrete request path.
    pub path: &'static str,

    /// Resource reference for the endpoint name.
    pub name: TextRef,

    /// Resource reference for the endpoint description.
    pub description: TextRef,

    /// Optional grouping label, a resource reference. Empty = uncategorized.
    pub category: TextRef,

    /// Every `#[authorize]` rule, in declaration order; empty = public.
    pub authorization: &'static [AuthorizeRule],

    /// `true` when the endpoint declared `#[authorize(dynamic)]`: the rules are
    /// the requirement handed to `IDynamicAuthorizer`, and the endpoint is
    /// authenticated (anonymous callers are rejected before the authorizer).
    pub dynamic: bool,

    /// `true` when the endpoint is not public (`#[authorize]` present, static or
    /// dynamic). An authenticated-only endpoint has `authorization` with one
    /// unconstrained rule.
    pub requires_auth: bool,

    /// OpenAPI response type name (e.g. `"UserModel"`, `"PagedResult<UserModel>"`).
    pub response_type: &'static str,
}

/// One endpoint with its name and description resolved for a culture.
///
/// This is the serializable shape for external systems; the field order below
/// is the JSON field order (serde derives it from the declaration order):
/// `id`, `guid`, `method`, `path`, `name`, `description`, `category`,
/// `authorization`, `dynamic`, `requires_auth`, `response_type`.
///
/// Both identity fields are always serialized:
/// * `guid` is the **primary key** for an external permission system — fixed
///   length, opaque, stable across route-pattern changes.
/// * `id` is the human-readable debug key. It contains the module path, so an
///   external system that must not leak internal module names should persist
///   `guid` only; `id` stays available for operator troubleshooting (logs,
///   `--doctor`, catalog lookups by [`IEndpointCatalog::get_by_id`]).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EndpointInfo {
    /// Stable endpoint identifier (qualified request type name); readable debug
    /// key, always serialized.
    pub id: &'static str,

    /// Deterministic GUID derived from [`Self::id`] (RFC 4122 v5); the
    /// preferred external primary key, always serialized.
    pub guid: String,

    /// Canonical upper-case HTTP method.
    pub method: &'static str,

    /// The declared route pattern.
    pub path: &'static str,

    /// Resolved endpoint name.
    pub name: String,

    /// Resolved endpoint description.
    pub description: String,

    /// Resolved grouping label; empty when the endpoint is uncategorized.
    pub category: String,

    /// The `#[authorize]` rule list, unchanged.
    pub authorization: &'static [AuthorizeRule],

    /// `true` when the endpoint declared `#[authorize(dynamic)]`.
    pub dynamic: bool,

    /// `true` when the endpoint is not public.
    pub requires_auth: bool,

    /// OpenAPI response type name.
    pub response_type: &'static str,
}

/// Runtime endpoint metadata source.
///
/// Resolved from the container (`Arc<dyn IEndpointCatalog>`) or from
/// `Host::endpoints()`. Equivalent to ASP.NET Core's `EndpointDataSource`: the
/// list is read-only and fixed for the lifetime of the host.
pub trait IEndpointCatalog: Send + Sync {
    /// Every endpoint, sorted by `path` then `method`.
    ///
    /// `path` is the original route pattern string from the declaration.
    fn all(&self) -> &[EndpointDescriptor];

    /// Look up one endpoint by its canonical method and declared route pattern.
    fn get(&self, method: &str, path: &str) -> Option<&EndpointDescriptor> {
        self.all()
            .iter()
            .find(|descriptor| descriptor.method == method && descriptor.path == path)
    }

    /// Look up one endpoint by its stable [`EndpointDescriptor::id`].
    fn get_by_id(&self, id: &str) -> Option<&EndpointDescriptor> {
        self.all().iter().find(|descriptor| descriptor.id == id)
    }

    /// Look up one endpoint by its deterministic GUID.
    fn get_by_guid(&self, guid: EndpointGuid) -> Option<&EndpointDescriptor> {
        self.all().iter().find(|descriptor| descriptor.guid == guid)
    }

    /// Resolve every endpoint's text for the localizer's default culture.
    ///
    /// The order matches [`Self::all`], so syncing the result into a database is
    /// idempotent.
    fn localized(&self, localizer: &dyn IStringLocalizer) -> Vec<EndpointInfo> {
        self.all()
            .iter()
            .map(|descriptor| EndpointInfo {
                id: descriptor.id,
                guid: descriptor.guid.to_string(),
                method: descriptor.method,
                path: descriptor.path,
                name: localizer.resolve(descriptor.name).value,
                description: localizer.resolve(descriptor.description).value,
                category: localizer.resolve(descriptor.category).value,
                authorization: descriptor.authorization,
                dynamic: descriptor.dynamic,
                requires_auth: descriptor.requires_auth,
                response_type: descriptor.response_type,
            })
            .collect()
    }

    /// Resolve every endpoint's text for an explicit culture.
    ///
    /// Use the default [`Self::localized`] for administrative output; this
    /// variant is for deliberately producing another language's view.
    fn localized_for(&self, localizer: &dyn IStringLocalizer, culture: &str) -> Vec<EndpointInfo> {
        self.all()
            .iter()
            .map(|descriptor| EndpointInfo {
                id: descriptor.id,
                guid: descriptor.guid.to_string(),
                method: descriptor.method,
                path: descriptor.path,
                name: localizer.resolve_in(culture, descriptor.name).value,
                description: localizer.resolve_in(culture, descriptor.description).value,
                category: localizer.resolve_in(culture, descriptor.category).value,
                authorization: descriptor.authorization,
                dynamic: descriptor.dynamic,
                requires_auth: descriptor.requires_auth,
                response_type: descriptor.response_type,
            })
            .collect()
    }
}

/// Every endpoint declared by a route macro.
pub struct EndpointCatalog {
    descriptors: Vec<EndpointDescriptor>,
}

impl EndpointCatalog {
    /// Build the catalog from the compile-time route table.
    ///
    /// This is the historic name of [`Self::try_from_inventory`] and is kept
    /// for existing call sites; both are fallible, so a duplicate endpoint
    /// identity is never silently accepted (and never panics).
    ///
    /// `HostBuilder::try_build()` calls this once and reports the failure as
    /// `Error::di`; application code receives the built catalog through DI or
    /// `Host::endpoints()`.
    pub fn from_inventory() -> crate::error::Result<Self> {
        Self::try_from_inventory()
    }

    /// Build the catalog, failing with a diagnosable `Error::di` when two
    /// routes declare the same [`EndpointDescriptor::id`].
    ///
    /// A permission catalog is keyed by `id`/`guid`, so two routes with one
    /// identity would make the key ambiguous. The error message is
    /// operator-facing (log/startup output) and lists every duplicate `id`
    /// with its conflicting `METHOD /path` declarations; the HTTP response
    /// never exposes it (`public_detail() == None`).
    pub fn try_from_inventory() -> crate::error::Result<Self> {
        let duplicates = duplicate_endpoint_ids();
        if !duplicates.is_empty() {
            return Err(duplicate_endpoint_id_error(&duplicates));
        }

        let mut descriptors: Vec<EndpointDescriptor> = inventory::iter::<RouteEntry>()
            .map(|entry| EndpointDescriptor {
                id: entry.request_id,
                guid: EndpointGuid::from_request_id(entry.request_id),
                method: entry.method.as_str(),
                path: entry.path,
                name: entry.name,
                description: entry.description,
                category: entry.category,
                authorization: entry.authorization,
                dynamic: entry.authorization_dynamic,
                requires_auth: entry.authorization_dynamic || !entry.authorization.is_empty(),
                response_type: entry.rsp_type,
            })
            .collect();
        descriptors.sort_by(|a, b| a.path.cmp(b.path).then_with(|| a.method.cmp(b.method)));
        Ok(Self { descriptors })
    }
}

/// Route declarations grouped by a request identity that appears more than once.
///
/// Each row is `(id, [(method, path), …])`; rows are sorted by `id` and the
/// declarations by `(method, path)`, so the diagnostic is deterministic.
/// [`EndpointCatalog::try_from_inventory`] and
/// [`format_route_diagnostics`](crate::route::diagnostics::format_route_diagnostics)
/// share this one rule.
pub(crate) fn duplicate_endpoint_ids() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    let mut by_id: HashMap<&'static str, Vec<(&'static str, &'static str)>> = HashMap::new();
    for entry in inventory::iter::<RouteEntry>() {
        by_id
            .entry(entry.request_id)
            .or_default()
            .push((entry.method.as_str(), entry.path));
    }

    let mut duplicates: Vec<(&'static str, Vec<(&'static str, &'static str)>)> = by_id
        .into_iter()
        .filter(|(_, declarations)| declarations.len() > 1)
        .collect();
    duplicates.sort_by_key(|(id, _)| *id);
    for (_, declarations) in &mut duplicates {
        declarations.sort_unstable();
    }
    duplicates
}

/// Format the duplicate-identity report as an operator-facing `Error::di`.
fn duplicate_endpoint_id_error(
    duplicates: &[(&'static str, Vec<(&'static str, &'static str)>)],
) -> Error {
    let mut message = format!(
        "Duplicate endpoint id(s) ({}). Each request type must declare exactly one route: the \
         endpoint catalog keys permission entries by `id`/`guid`, so a second declaration would \
         make the key ambiguous:",
        duplicates.len()
    );
    for (id, declarations) in duplicates {
        let declared_by = declarations
            .iter()
            .map(|(method, path)| format!("{method} {path}"))
            .collect::<Vec<_>>()
            .join(", ");
        message.push_str(&format!("\n- `{id}`: {declared_by}"));
    }
    message.push_str(
        "\nFix: keep one route macro per request type (or give each hand-written fixture a \
         distinct `request_id`).",
    );
    Error::di(message)
}

impl IEndpointCatalog for EndpointCatalog {
    fn all(&self) -> &[EndpointDescriptor] {
        &self.descriptors
    }
}
