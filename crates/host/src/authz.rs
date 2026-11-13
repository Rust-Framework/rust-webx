//! Resource-based authorization module for the rust-webx framework.
//!
//! Provides:
//! - `ResourceAuthorization` —an `IAuthorizationPolicy` that checks
//!   whether a user's roles or permissions grant access to a specific
//!   resource (identified by the matched route pattern).
//! - `AuthorizerSet` —collects all `IDynamicAuthorizer` instances from DI
//!   and runs them in sequence on protected routes.
//!
//! # Example
//!
//! ```ignore
//! use webx_host::authz::{ResourceAuthorization, resource_auth_middleware};
//!
//! let policy = ResourceAuthorization::new()
//!     .allow_role("/api/users", "admin")
//!     .allow_role("/api/users/{id}", "user")
//!     .allow_permission("/api/admin/**", "admin:access");
//!
//! let middleware = resource_auth_middleware(Arc::new(policy));
//! ```
//!
//! # Dynamic authorizer (preferred)
//!
//! ```ignore
//! use webx_core::auth::{IClaims, IDynamicAuthorizer};
//!
//! #[derive(Default)]
//! struct RoleAuthorizer;
//!
//! #[async_trait::async_trait]
//! impl IDynamicAuthorizer for RoleAuthorizer {
//!     async fn authorize(&self, claims: &dyn IClaims, pattern: &str, method: &str) -> Result<()> {
//!         // your custom logic
//!         Ok(())
//!     }
//! }
//! ```
//!
//! Then register in DI:
//! ```ignore
//! svc.singleton::<dyn IDynamicAuthorizer>(|_| Arc::new(RoleAuthorizer::default()))
//! ```

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::Arc;
use webx_core::auth::{DeclaredRules, IAuthorizationPolicy, IClaims, IDynamicAuthorizer};
use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;

// ---------------------------------------------------------------------------
// ResourceAuthorization —IAuthorizationPolicy implementation
// ---------------------------------------------------------------------------

/// Authorization policy that maps route patterns to allowed roles and permissions.
///
/// * `resource_key` →`allowed_roles`: users with ANY of these roles are granted access.
/// * `resource_key` →`required_permissions`: users with ANY of these permissions are granted access.
///
/// A user is authorized if they have at least one matching role OR at least one
/// matching permission.
pub struct ResourceAuthorization {
    allowed_roles: HashMap<String, Vec<String>>,
    required_permissions: HashMap<String, Vec<String>>,
}

impl ResourceAuthorization {
    /// Create a new, empty authorization policy.
    pub fn new() -> Self {
        Self {
            allowed_roles: HashMap::new(),
            required_permissions: HashMap::new(),
        }
    }

    /// Allow a specific role to access a resource.
    ///
    /// `resource_key` is the route pattern (e.g., `"/api/users"`).
    pub fn allow_role(mut self, resource_key: impl Into<String>, role: impl Into<String>) -> Self {
        self.allowed_roles
            .entry(resource_key.into())
            .or_default()
            .push(role.into());
        self
    }

    /// Require a specific permission to access a resource.
    pub fn allow_permission(
        mut self,
        resource_key: impl Into<String>,
        permission: impl Into<String>,
    ) -> Self {
        self.required_permissions
            .entry(resource_key.into())
            .or_default()
            .push(permission.into());
        self
    }
}

impl Default for ResourceAuthorization {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceAuthorization {
    /// Returns true when this policy has explicit rules for `resource_key`.
    pub fn covers_route(&self, resource_key: &str) -> bool {
        self.allowed_roles.contains_key(resource_key)
            || self.required_permissions.contains_key(resource_key)
    }
}

#[async_trait::async_trait]
impl IAuthorizationPolicy for ResourceAuthorization {
    async fn authorize(
        &self,
        claims: &dyn IClaims,
        resource_key: &str,
        _method: &str,
    ) -> Result<()> {
        // Check role-based access
        if let Some(allowed) = self.allowed_roles.get(resource_key) {
            if allowed.iter().any(|r| claims.roles().contains(r)) {
                return Ok(());
            }
        }

        // Check permission-based access
        if let Some(required) = self.required_permissions.get(resource_key) {
            if required.iter().any(|p| claims.permissions().contains(p)) {
                return Ok(());
            }
        }

        // No matching policies found —deny by default
        Err(webx_core::error::Error::Forbidden(format!(
            "no policy grants access to '{} {}'",
            _method, resource_key
        )))
    }
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Authorization middleware that enforces an `IAuthorizationPolicy`.
///
/// Reads the authenticated claims and matched route pattern from the HTTP context
/// and calls `policy.authorize()`.
struct ResourceAuthMiddleware {
    policy: Arc<dyn IAuthorizationPolicy>,
}

#[async_trait::async_trait]
impl IMiddleware for ResourceAuthMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        // Get the matched route pattern (set by the router)
        let route_pattern = match ctx.request().route_pattern() {
            Some(p) => p.to_string(),
            None => {
                // No route matched —skip authorization (e.g., static files, health checks)
                return Ok(ControlFlow::Continue(()));
            }
        };

        let method = ctx.request().method().to_string();

        // Get claims from context (set by authentication middleware)
        // IHttpContext extends IClaimsExt, so claims() is available directly.
        match ctx.claims() {
            Some(claims) => self
                .policy
                .authorize(claims, &route_pattern, &method)
                .await
                .map(|_| ControlFlow::Continue(())),
            None => Err(webx_core::error::Error::Unauthorized(
                "authentication required".to_string(),
            )),
        }
    }
}

/// Create a resource-based authorization middleware.
///
/// Register this middleware AFTER the authentication middleware in the pipeline.
/// The middleware reads the route pattern (set by the router) and
/// claims (set by the auth middleware) to enforce the policy.
pub fn resource_auth_middleware(policy: Arc<dyn IAuthorizationPolicy>) -> impl IMiddleware {
    ResourceAuthMiddleware { policy }
}

// ---------------------------------------------------------------------------
// AuthorizerSet —dynamic authorizer collection
// ---------------------------------------------------------------------------

/// A set of `IDynamicAuthorizer` instances collected from the DI container.
///
/// Created automatically in `HostBuilder::build()` when one or more
/// `IDynamicAuthorizer` implementations are registered. If none are registered,
/// this is `None` and no dynamic authorization checks run.
#[derive(Clone)]
pub struct AuthorizerSet {
    authorizers: Vec<Arc<dyn IDynamicAuthorizer>>,
}

impl AuthorizerSet {
    /// Create a new set from a vector of authorizers.
    pub fn new(authorizers: Vec<Arc<dyn IDynamicAuthorizer>>) -> Self {
        Self { authorizers }
    }

    /// Run all registered authorizers in sequence.
    ///
    /// Returns `Ok(())` if ALL authorizers pass. Returns the first `Err` if any denies.
    ///
    /// **1.1 compatibility** — new code should call
    /// [`AuthorizerSet::authorize_with_rules`] instead, which passes the
    /// endpoint's declared rules. This method is equivalent to that call with an
    /// empty declaration.
    pub async fn authorize(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        method: &str,
    ) -> Result<()> {
        self.authorize_with_rules(claims, route_pattern, method, &DeclaredRules::empty())
            .await
    }

    /// Run all registered authorizers in sequence, handing them the matched
    /// endpoint's `#[authorize]` declaration.
    ///
    /// **Legacy since 1.3.0** — the host now calls
    /// [`AuthorizerSet::authorize_with_declared_rules`], which passes the raw
    /// `&'static [AuthorizeRule]` slice. This wrapper stays for source
    /// compatibility with 1.2.0 callers.
    ///
    /// Returns `Ok(())` if ALL authorizers pass. Returns the first `Err` if any denies.
    pub async fn authorize_with_rules(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        method: &str,
        declared: &DeclaredRules<'_>,
    ) -> Result<()> {
        for authorizer in &self.authorizers {
            authorizer
                .authorize_with_rules(claims, route_pattern, method, declared)
                .await?;
        }
        Ok(())
    }

    /// Run all registered authorizers in sequence, handing them the matched
    /// endpoint's `#[authorize]` declaration as the raw slice the route
    /// metadata owns (`RouteEntry.authorization`).
    ///
    /// This is the 1.3.0 primary path — zero allocation, and the declaration is
    /// the same `&'static [AuthorizeRule]` a dynamic endpoint delegates in full
    /// (`#[authorize(dynamic)]`). Implementations that override only the legacy
    /// `authorize` / `authorize_with_rules` methods keep working through the
    /// trait's default forwarding.
    ///
    /// Returns `Ok(())` if ALL authorizers pass. Returns the first `Err` if any denies.
    pub async fn authorize_with_declared_rules(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        method: &str,
        declared: &'static [webx_core::route::scan::AuthorizeRule],
    ) -> Result<()> {
        for authorizer in &self.authorizers {
            authorizer
                .authorize_with_declared_rules(claims, route_pattern, method, declared)
                .await?;
        }
        Ok(())
    }

    /// Returns `true` if there are no authorizers registered.
    pub fn is_empty(&self) -> bool {
        self.authorizers.is_empty()
    }
}

/// Result from collecting `IDynamicAuthorizer` instances from DI.
/// `None` means no authorizers are registered (pass-through mode).
pub fn collect_authorizers(provider: &rust_dix::ServiceProvider) -> Option<Arc<AuthorizerSet>> {
    let authorizers: Vec<Arc<dyn IDynamicAuthorizer>> = provider.get_all();
    if authorizers.is_empty() {
        None
    } else {
        Some(Arc::new(AuthorizerSet::new(authorizers)))
    }
}
