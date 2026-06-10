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
    async fn authorize(
        &self,
        claims: &dyn IClaims,
        resource_key: &str,
        method: &str,
    ) -> Result<()>;
}
