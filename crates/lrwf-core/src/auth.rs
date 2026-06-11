//! Authentication and authorization traits for the LRWF framework.
//!
//! Provides:
//! - `IClaims`: Extracted auth claims (JWT or other token types).
//! - `IAuthenticationHandler`: Authenticate an HTTP request and produce claims.
//! - `IAuthorizationPolicy`: Check if authenticated claims can access a resource.

use crate::error::Result;
use crate::http::IHttpContext;
use std::collections::HashMap;

/// Claims extracted from an authentication token (JWT, etc.).
///
/// Stored in `IHttpContext` extensions via `IClaimsExt`.
pub trait IClaims: Send + Sync {
    /// The user / subject identifier.
    fn subject(&self) -> &str;

    /// Roles assigned to the user.
    fn roles(&self) -> &[String];

    /// Permissions assigned to the user.
    fn permissions(&self) -> &[String];

    /// Raw claims map (key-value pairs from the token).
    fn claims(&self) -> &HashMap<String, String>;

    /// Clone the claims into a new boxed trait object.
    fn clone_box(&self) -> Box<dyn IClaims>;

    // ── Convenience helpers (default implementations) ──

    /// Check whether a specific role is assigned.
    ///
    /// ```ignore
    /// if claims.has_role("admin") { ... }
    /// ```
    fn has_role(&self, role: &str) -> bool {
        self.roles().iter().any(|r| r == role)
    }

    /// Alias for `subject()` — returns the user identifier.
    fn get_userid(&self) -> &str {
        self.subject()
    }

    /// Read the display name from the raw claims map (key `"name"`).
    /// Returns `None` when absent.
    fn get_username(&self) -> Option<&str> {
        self.claims().get("name").map(|s| s.as_str())
    }

    /// Read the tenant identifier from the raw claims map (key `"tenant_id"`).
    /// Returns `None` when absent.
    fn get_tenantid(&self) -> Option<&str> {
        self.claims()
            .get("tenant_id")
            .or_else(|| self.claims().get("tenant"))
            .map(|s| s.as_str())
    }
}

impl Clone for Box<dyn IClaims> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// Authentication scheme interface.
///
/// Implementations read credentials from the HTTP context (e.g., JWT bearer token,
/// API key header, cookie) and return claims or `None` if unauthenticated.
#[async_trait::async_trait]
pub trait IAuthenticationHandler: Send + Sync {
    /// Authenticate the request and return claims, or `None` if not authenticated.
    ///
    /// Uses `&mut dyn IHttpContext` so the returned future is `Send`
    /// (required by `tokio::spawn` — `&dyn IHttpContext` is `!Send`
    /// because `IHttpContext` is not `Sync`).
    async fn authenticate(&self, ctx: &mut dyn IHttpContext) -> Result<Option<Box<dyn IClaims>>>;
}

/// Authorization policy that checks whether an authenticated user
/// can access a given resource.
///
/// The `resource_key` is the original route pattern string (e.g., `"/api/users/{id}"`),
/// enabling dynamic identity-authorization-data systems to match routes directly.
#[async_trait::async_trait]
pub trait IAuthorizationPolicy: Send + Sync {
    /// Check if the authenticated user can access the resource.
    ///
    /// * `claims`  — the user's authentication claims.
    /// * `resource_key` — the original route pattern string.
    /// * `method` — the HTTP method.
    ///
    /// Returns `Ok(())` if authorized, or an `Err` if forbidden.
    async fn authorize(&self, claims: &dyn IClaims, resource_key: &str, method: &str)
        -> Result<()>;
}

/// Dynamic authorizer interface — pluggable authorization for protected routes.
///
/// Implement this trait and register it in the DI container:
///
/// ```ignore
/// svc.singleton::<dyn IDynamicAuthorizer>(|_| Arc::new(MyAuthorizer::default()))
/// ```
///
/// The framework automatically detects registered `IDynamicAuthorizer` implementations
/// and invokes them on every route that has `#[authorize]`.
/// If no implementations are registered, authorization is pass-through (no dynamic checks).
///
/// # Method
///
/// * `authorize` — receives the user's claims, matched route pattern, and HTTP method.
///   Returns `Ok(())` if allowed, or `Err` with details if denied.
#[async_trait::async_trait]
pub trait IDynamicAuthorizer: Send + Sync {
    /// Validate whether the authenticated user can access the resource.
    async fn authorize(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        method: &str,
    ) -> Result<()>;
}
