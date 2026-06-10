//! Resource-based authorization module for the LRWF framework.
//!
//! Provides `ResourceAuthorization` — an `IAuthorizationPolicy` that checks
//! whether a user's roles or permissions grant access to a specific
//! resource (identified by the matched route pattern).
//!
//! # Example
//!
//! ```ignore
//! use lrwf_http::authz::{ResourceAuthorization, resource_auth_middleware};
//!
//! let policy = ResourceAuthorization::new()
//!     .allow_role("/api/users", "admin")
//!     .allow_role("/api/users/{id}", "user")
//!     .allow_permission("/api/admin/**", "admin:access");
//!
//! let middleware = resource_auth_middleware(Arc::new(policy));
//! ```

use lrwf_core::auth::{IAuthorizationPolicy, IClaims};
use lrwf_core::error::Result;
use lrwf_core::http::IHttpContext;
use lrwf_core::middleware::IMiddleware;
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// ResourceAuthorization — IAuthorizationPolicy implementation
// ---------------------------------------------------------------------------

/// Authorization policy that maps route patterns to allowed roles and permissions.
///
/// * `resource_key` → `allowed_roles`: users with ANY of these roles are granted access.
/// * `resource_key` → `required_permissions`: users with ANY of these permissions are granted access.
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

        // No matching policies found — deny by default
        Err(lrwf_core::error::Error::Http(format!(
            "Forbidden: no policy grants access to '{} {}'",
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
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        // Get the matched route pattern (set by the router)
        let route_pattern = match ctx.request().route_pattern() {
            Some(p) => p.to_string(),
            None => {
                // No route matched — skip authorization (e.g., static files, health checks)
                return Ok(());
            }
        };

        let method = ctx.request().method().to_string();

        // Get claims from context (set by authentication middleware)
        // IHttpContext extends IClaimsExt, so claims() is available directly.
        match ctx.claims() {
            Some(claims) => self.policy.authorize(claims, &route_pattern, &method).await,
            None => Err(lrwf_core::error::Error::Http(
                "Unauthorized: no authentication claims found".to_string(),
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
