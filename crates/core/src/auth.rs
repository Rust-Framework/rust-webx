//! Authentication and authorization traits for the rust-webx framework.
//!
//! Provides:
//! - `IClaims`: Extracted auth claims (JWT or other token types).
//! - `IAuthenticationHandler`: Authenticate an HTTP request and produce claims.
//! - `IAuthorizationPolicy`: Check if authenticated claims can access a resource.

use crate::error::{Error, Result};
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

    // â”€â”€ Convenience helpers (default implementations) â”€â”€

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

    /// Read the email address from the raw claims map (key `"email"`).
    /// Returns `None` when absent.
    fn get_email(&self) -> Option<&str> {
        self.claims().get("email").map(|s| s.as_str())
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

/// The standard identity a request can carry.
///
/// Put it in a request with `#[fromclaim]` and the binder fills it from the
/// authenticated principal:
///
/// ```ignore
/// #[derive(WebxRequestMeta)]
/// pub struct CreateOrderRequest {
///     #[fromclaim]
///     pub user: CurrentUser,
///     pub sku: String,
/// }
/// ```
///
/// The handler then reads `req.user.id`, `req.user.tenant_id` or
/// `req.user.is_in_role("admin")` without touching the token or the context.
/// `Option<CurrentUser>` makes the endpoint work for anonymous callers too.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CurrentUser {
    /// The subject (`sub`) — the stable user identifier.
    pub id: String,
    /// Display name (`name` claim), empty when the token has none.
    pub name: String,
    /// Email address (`email` claim), empty when the token has none.
    pub email: String,
    /// Tenant identifier (`tenant_id` / `tenant`), empty for single-tenant apps.
    pub tenant_id: String,
    /// Roles assigned to the user.
    pub roles: Vec<String>,
    /// Fine-grained permissions assigned to the user.
    pub permissions: Vec<String>,
}

impl CurrentUser {
    /// Build the identity from an authenticated principal.
    pub fn from_claims(claims: &dyn IClaims) -> Self {
        Self {
            id: claims.subject().to_string(),
            name: claims.get_username().unwrap_or_default().to_string(),
            email: claims.get_email().unwrap_or_default().to_string(),
            tenant_id: claims.get_tenantid().unwrap_or_default().to_string(),
            roles: claims.roles().to_vec(),
            permissions: claims.permissions().to_vec(),
        }
    }

    /// Whether the user has `role`.
    pub fn is_in_role(&self, role: &str) -> bool {
        self.roles.iter().any(|assigned| assigned == role)
    }

    /// Whether the user has `permission`.
    pub fn has_permission(&self, permission: &str) -> bool {
        self.permissions
            .iter()
            .any(|assigned| assigned == permission)
    }

    /// The display name, falling back to the identifier.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
}

/// Convenience accessors for a request that carries a [`CurrentUser`].
///
/// `#[derive(WebxRequestMeta)]` implements this automatically when the struct
/// has a `#[fromclaim]` field of type `CurrentUser` or `Option<CurrentUser>`.
pub trait RequestUser {
    /// The authenticated identity, or `None` for an anonymous request.
    fn current_user(&self) -> Option<&CurrentUser>;

    /// The subject identifier, or `None` when anonymous.
    fn user_id(&self) -> Option<&str> {
        self.current_user().map(|user| user.id.as_str())
    }

    /// The display name, or `None` when anonymous.
    fn user_name(&self) -> Option<&str> {
        self.current_user().map(|user| user.name.as_str())
    }

    /// The email address, or `None` when anonymous.
    fn user_email(&self) -> Option<&str> {
        self.current_user().map(|user| user.email.as_str())
    }

    /// The tenant identifier, or `None` when anonymous.
    fn tenant_id(&self) -> Option<&str> {
        self.current_user().map(|user| user.tenant_id.as_str())
    }

    /// The roles of the authenticated user (empty when anonymous).
    fn roles(&self) -> &[String] {
        match self.current_user() {
            Some(user) => &user.roles,
            None => &[],
        }
    }

    /// Whether the authenticated user has `role`.
    fn is_in_role(&self, role: &str) -> bool {
        self.current_user()
            .map(|user| user.is_in_role(role))
            .unwrap_or(false)
    }

    /// The identity, or `401 Unauthorized` when the request is anonymous.
    fn require_user(&self) -> Result<&CurrentUser> {
        self.current_user().ok_or_else(|| {
            Error::Unauthorized("authentication required for this endpoint".to_string())
        })
    }
}
