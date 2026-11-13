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

/// Role / permission codes declared on the matched endpoint by `#[authorize]`.
///
/// The framework hands this to [`IDynamicAuthorizer::authorize_with_rules`] so a
/// dynamic backend sees the *same literals* the endpoint declared — e.g. an
/// endpoint annotated `#[authorize(permission = "order.create")]` yields
/// `permissions == ["order.create"]`. Without it a backend can only recover the
/// requirement by re-mapping route + method, which duplicates the declaration.
///
/// Both slices are empty when the endpoint declares no role/permission
/// constraint (a bare `#[authorize]` or `AuthorizeRule::authenticated()`).
///
/// New in 1.2.0; additive — `authorize` keeps its 1.1 signature.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeclaredRules<'a> {
    /// Any-of role names declared on the endpoint; empty = no role constraint.
    pub roles: &'a [&'a str],

    /// Any-of permission names declared on the endpoint; empty = no permission
    /// constraint.
    pub permissions: &'a [&'a str],
}

impl<'a> DeclaredRules<'a> {
    /// An endpoint with no declared role/permission constraint.
    pub const fn empty() -> Self {
        Self {
            roles: &[],
            permissions: &[],
        }
    }

    /// Build from the declared role and permission codes.
    pub const fn new(roles: &'a [&'a str], permissions: &'a [&'a str]) -> Self {
        Self { roles, permissions }
    }

    /// Whether neither a role nor a permission was declared.
    pub fn is_empty(&self) -> bool {
        self.roles.is_empty() && self.permissions.is_empty()
    }
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
/// # Methods
///
/// * `authorize` — receives the user's claims, matched route pattern, and HTTP
///   method. Returns `Ok(())` if allowed, or `Err` with details if denied.
///   Kept for source compatibility with 1.1.
/// * `authorize_with_rules` — same, plus the endpoint's [`DeclaredRules`]
///   (the role/permission literals written in `#[authorize]`). The framework
///   calls this one; the default implementation forwards to `authorize`, so an
///   implementation that only overrides `authorize` keeps working unchanged.
#[async_trait::async_trait]
pub trait IDynamicAuthorizer: Send + Sync {
    /// Validate whether the authenticated user can access the resource.
    async fn authorize(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        method: &str,
    ) -> Result<()>;

    /// Validate access with the endpoint's declared role / permission codes.
    ///
    /// Override this instead of `authorize` when the authorization backend must
    /// know *which* permission the endpoint declared (resource-instance checks,
    /// permission-to-policy mapping, audit). Calls made through this method never
    /// reach the overridden `authorize` when this one is overridden.
    ///
    /// The default forwards to [`IDynamicAuthorizer::authorize`], which keeps
    /// 1.1 implementations wire-compatible.
    async fn authorize_with_rules(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        method: &str,
        _declared: &DeclaredRules<'_>,
    ) -> Result<()> {
        self.authorize(claims, route_pattern, method).await
    }
}

/// The standard identity a request can carry.
///
/// `IdentityUser` is a **read-only projection of [`IClaims`]** for request
/// handling — not a persistence entity. When an endpoint needs the raw claim
/// map, read it through [`IClaimsExt`](crate::http::IClaimsExt) in middleware or
/// infrastructure code; `IdentityUser` covers the identity fields almost every
/// handler needs.
///
/// Put it in a request with `#[fromclaim]` and the binder fills it from the
/// authenticated principal:
///
/// ```ignore
/// #[derive(WebxRequestMeta)]
/// pub struct CreateOrderRequest {
///     #[fromclaim]
///     pub user: IdentityUser,
///     pub sku: String,
/// }
/// ```
///
/// The handler then reads `req.user.id`, `req.user.tenant_id` or
/// `req.user.is_in_role("admin")` without touching the token or the context.
/// `#[fromclaim(optional)] viewer: Option<IdentityUser>` makes the endpoint work
/// for anonymous callers too; anonymous access must be declared explicitly.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IdentityUser {
    /// The subject (`sub`) — the stable user identifier.
    pub id: String,
    /// Display name (`name` claim), empty when the token has none.
    pub name: String,
    /// Email address (`email` claim), empty when the token has none.
    pub email: String,
    /// Tenant identifier (`tenant_id` / `tenant`).
    ///
    /// The **empty string means "no tenant claim"** — tokens from single-tenant
    /// apps and tokens without the claim leave it empty. Use
    /// [`IdentityUser::tenant`] when the difference between "no tenant claim"
    /// and "declared empty" matters, or [`IdentityUser::require_tenant`] to
    /// reject a principal without a tenant.
    pub tenant_id: String,
    /// Roles assigned to the user.
    pub roles: Vec<String>,
    /// Fine-grained permissions assigned to the user.
    pub permissions: Vec<String>,
}

impl IdentityUser {
    /// Build the identity from an authenticated principal.
    ///
    /// Absent `name` / `email` / `tenant_id` claims become empty strings, so an
    /// empty [`IdentityUser::tenant_id`] means "the principal carries no tenant
    /// claim" rather than a tenant literally named `""`.
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

    /// The tenant identifier, or `None` when the principal carries no tenant
    /// claim.
    ///
    /// [`IdentityUser::tenant_id`] stores "no claim" as the empty string for
    /// backwards compatibility; this accessor makes that distinction explicit.
    pub fn tenant(&self) -> Option<&str> {
        if self.tenant_id.is_empty() {
            None
        } else {
            Some(self.tenant_id.as_str())
        }
    }

    /// The tenant identifier, or `Error::Forbidden` when the principal carries
    /// no tenant claim.
    ///
    /// A missing tenant is **not** an authentication failure — the principal
    /// already proved who it is — it is a scope problem: the token simply does
    /// not reach any tenant. Reporting 403 instead of 401 keeps clients from
    /// retrying with the same valid credentials.
    pub fn require_tenant(&self) -> Result<&str> {
        self.tenant().ok_or_else(|| {
            Error::Forbidden("the authenticated principal has no tenant claim".to_string())
        })
    }

    /// Whether the user has `role`.
    ///
    /// Boolean check only — for a 401/403 decision use
    /// [`RequestUser::require_role`] instead.
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

/// Convenience accessors for a request that carries an [`IdentityUser`].
///
/// `#[derive(WebxRequestMeta)]` implements this automatically when the struct
/// has a `#[fromclaim]` field of type `IdentityUser` (or
/// `#[fromclaim(optional)]` of type `Option<IdentityUser>`).
pub trait RequestUser {
    /// The authenticated identity, or `None` for an anonymous request.
    fn current_user(&self) -> Option<&IdentityUser>;

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
    ///
    /// Returns `Some("")` for an authenticated principal without a tenant claim
    /// (that is how [`IdentityUser::tenant_id`] represents "no claim"); use
    /// [`IdentityUser::tenant`] or [`RequestUser::require_tenant`] when the
    /// difference matters.
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

    /// The permissions of the authenticated user (empty when anonymous).
    fn permissions(&self) -> &[String] {
        match self.current_user() {
            Some(user) => &user.permissions,
            None => &[],
        }
    }

    /// Whether the authenticated user has `role`.
    ///
    /// Boolean branch only (anonymous → `false`); it cannot tell "not signed in"
    /// from "signed in without the role". Use [`RequestUser::require_role`] for
    /// a 401/403 decision.
    fn is_in_role(&self, role: &str) -> bool {
        self.current_user()
            .map(|user| user.is_in_role(role))
            .unwrap_or(false)
    }

    /// Whether the authenticated user has `permission` (anonymous → `false`).
    fn has_permission(&self, permission: &str) -> bool {
        self.current_user()
            .map(|user| user.has_permission(permission))
            .unwrap_or(false)
    }

    /// The identity, or `401 Unauthorized` when the request is anonymous.
    fn require_user(&self) -> Result<&IdentityUser> {
        self.current_user()
            .ok_or_else(|| Error::Unauthorized("authentication required".to_string()))
    }

    /// Require `role`, separating "not signed in" from "not allowed".
    ///
    /// * anonymous → `401 Unauthorized` (`authentication required`)
    /// * authenticated without the role → `403 Forbidden` (`{role} role required`)
    ///
    /// The 401/403 split lets clients react correctly: acquire a token, or stop
    /// retrying because the token lacks the role.
    fn require_role(&self, role: &str) -> Result<()> {
        let user = self.require_user()?;
        if user.is_in_role(role) {
            Ok(())
        } else {
            Err(Error::Forbidden(format!("{role} role required")))
        }
    }

    /// Require a tenant claim, separating "not signed in" from "no tenant
    /// scope".
    ///
    /// * anonymous → `401 Unauthorized`
    /// * authenticated without a tenant claim → delegates to
    ///   [`IdentityUser::require_tenant`] (`403 Forbidden`)
    fn require_tenant(&self) -> Result<&str> {
        self.require_user()?.require_tenant()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_rules_empty_is_both_dimensions_empty() {
        let empty = DeclaredRules::empty();
        assert!(empty.is_empty());
        assert!(empty.roles.is_empty());
        assert!(empty.permissions.is_empty());
        assert_eq!(empty, DeclaredRules::default());
    }

    #[test]
    fn declared_rules_keep_the_declared_literals() {
        let declared = DeclaredRules::new(&["admin"], &["order.create"]);
        assert_eq!(declared.roles, &["admin"]);
        assert_eq!(declared.permissions, &["order.create"]);
        assert!(!declared.is_empty());
    }

    /// Minimal `RequestUser` carrier: a request struct with
    /// `#[fromclaim(optional)] viewer: Option<IdentityUser>` reduces to this.
    struct TestRequest<'a>(Option<&'a IdentityUser>);

    impl RequestUser for TestRequest<'_> {
        fn current_user(&self) -> Option<&IdentityUser> {
            self.0
        }
    }

    fn multi_tenant_admin() -> IdentityUser {
        IdentityUser {
            id: "user-1".into(),
            name: "Ada".into(),
            roles: vec!["admin".into(), "user".into()],
            permissions: vec!["orders:read".into()],
            tenant_id: "acme".into(),
            ..Default::default()
        }
    }

    /// Authenticated principal from a token without a tenant claim.
    fn single_tenant_user() -> IdentityUser {
        IdentityUser {
            id: "user-2".into(),
            roles: Vec::new(),
            permissions: Vec::new(),
            tenant_id: String::new(),
            ..Default::default()
        }
    }

    #[test]
    fn identity_tenant_is_none_when_claim_is_empty() {
        assert_eq!(multi_tenant_admin().tenant(), Some("acme"));
        assert_eq!(
            single_tenant_user().tenant(),
            None,
            "empty tenant_id means 'no tenant claim', not a tenant named \"\""
        );
    }

    #[test]
    fn identity_require_tenant_is_forbidden_without_claim() {
        let user = single_tenant_user();
        let err = user.require_tenant().unwrap_err();
        assert!(
            matches!(&err, Error::Forbidden(detail) if detail.contains("no tenant claim")),
            "expected Forbidden(no tenant claim), got {err:?}"
        );
        assert_eq!(err.status_code(), 403, "missing tenant scope is not 401");

        assert_eq!(multi_tenant_admin().require_tenant().unwrap(), "acme");
    }

    #[test]
    fn anonymous_request_user_is_empty_and_unauthorized() {
        let anonymous = TestRequest(None);

        assert!(anonymous.permissions().is_empty());
        assert!(!anonymous.has_permission("orders:read"));
        assert!(!anonymous.is_in_role("admin"));
        assert!(anonymous.current_user().is_none());

        let role_err = anonymous.require_role("admin").unwrap_err();
        assert!(
            matches!(&role_err, Error::Unauthorized(detail) if detail == "authentication required"),
            "anonymous must be 401, got {role_err:?}"
        );
        assert_eq!(role_err.status_code(), 401);

        let tenant_err = anonymous.require_tenant().unwrap_err();
        assert_eq!(tenant_err.status_code(), 401);
    }

    #[test]
    fn authenticated_request_user_exposes_roles_permissions_and_tenant() {
        let user = multi_tenant_admin();
        let request = TestRequest(Some(&user));

        assert_eq!(request.permissions().len(), 1);
        assert_eq!(request.permissions()[0], "orders:read");
        assert!(request.has_permission("orders:read"));
        assert!(!request.has_permission("orders:write"));
        assert!(request.is_in_role("admin"));
        assert!(request.require_role("admin").is_ok());
        assert_eq!(request.require_tenant().unwrap(), "acme");
    }

    #[test]
    fn require_role_is_forbidden_when_role_is_missing() {
        let user = multi_tenant_admin();
        let request = TestRequest(Some(&user));

        let err = request.require_role("auditor").unwrap_err();
        assert!(
            matches!(&err, Error::Forbidden(detail) if detail == "auditor role required"),
            "expected Forbidden(auditor role required), got {err:?}"
        );
        assert_eq!(err.status_code(), 403);
    }

    #[test]
    fn require_tenant_is_forbidden_for_authenticated_principal_without_claim() {
        let user = single_tenant_user();
        let request = TestRequest(Some(&user));

        // Still signed in: this is a scope problem, not a credentials problem.
        assert!(request.current_user().is_some());
        let err = request.require_tenant().unwrap_err();
        assert!(matches!(&err, Error::Forbidden(_)), "got {err:?}");
        assert_eq!(err.status_code(), 403);
    }
}
