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

use crate::localization::{IStringLocalizer, TextRef};
use crate::route::scan::{AuthorizeRule, RouteEntry};
use serde::Serialize;

/// One endpoint's language-neutral metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EndpointDescriptor {
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
}

/// One endpoint with its name and description resolved for a culture.
///
/// This is the serializable shape for external systems. Permissions should be
/// keyed on `(method, path)`, which is the route pattern runtime authorization
/// already sees.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EndpointInfo {
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

    /// Resolve every endpoint's text for the localizer's default culture.
    ///
    /// The order matches [`Self::all`], so syncing the result into a database is
    /// idempotent.
    fn localized(&self, localizer: &dyn IStringLocalizer) -> Vec<EndpointInfo> {
        self.all()
            .iter()
            .map(|descriptor| EndpointInfo {
                method: descriptor.method,
                path: descriptor.path,
                name: localizer.resolve(descriptor.name).value,
                description: localizer.resolve(descriptor.description).value,
                category: localizer.resolve(descriptor.category).value,
                authorization: descriptor.authorization,
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
                method: descriptor.method,
                path: descriptor.path,
                name: localizer.resolve_in(culture, descriptor.name).value,
                description: localizer.resolve_in(culture, descriptor.description).value,
                category: localizer.resolve_in(culture, descriptor.category).value,
                authorization: descriptor.authorization,
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
    /// `Host::build()` calls this once; application code receives the result
    /// through DI or `Host::endpoints()`.
    pub fn from_inventory() -> Self {
        let mut descriptors: Vec<EndpointDescriptor> = inventory::iter::<RouteEntry>()
            .map(|entry| EndpointDescriptor {
                method: entry.method.as_str(),
                path: entry.path,
                name: entry.name,
                description: entry.description,
                category: entry.category,
                authorization: entry.authorization,
            })
            .collect();
        descriptors.sort_by(|a, b| a.path.cmp(b.path).then_with(|| a.method.cmp(b.method)));
        Self { descriptors }
    }
}

impl IEndpointCatalog for EndpointCatalog {
    fn all(&self) -> &[EndpointDescriptor] {
        &self.descriptors
    }
}
