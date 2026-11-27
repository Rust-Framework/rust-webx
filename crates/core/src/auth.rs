//! Authentication and authorization traits for the rust-webx framework.
//!
//! Provides:
//! - `IClaims`: Extracted auth claims (JWT or other token types).
//! - `IAuthenticationHandler`: Authenticate an HTTP request and produce claims.

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

    /// The token identifier (`jti` claim), when the token carries one.
    ///
    /// The authentication middleware looks this up in
    /// [`ISessionStore`](crate::security::ISessionStore) to reject revoked
    /// tokens. A token without a `jti` cannot be revoked individually; the
    /// revocation check then leaves it alone (it still expires at `exp`).
    fn jti(&self) -> Option<&str> {
        self.claims().get("jti").map(String::as_str)
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

/// A pluggable authentication scheme that participates in the host's scheme
/// chain.
///
/// The host tries schemes in registration order: the first scheme whose
/// [`matches`](Self::matches) returns `true` handles the request. A scheme that
/// matches but produces no claims leaves the request anonymous (JWT semantics);
/// a scheme can also return [`Error::unauthorized`] to reject malformed
/// credentials outright (API key semantics).
///
/// `IAuthenticationHandler` remains the lower-level "verify a credential"
/// port; `IAuthenticationScheme` adds the cheap shape check that lets several
/// credential types (JWT, `sk-` API keys, …) coexist on one host.
#[async_trait::async_trait]
pub trait IAuthenticationScheme: Send + Sync {
    /// Stable scheme name for diagnostics, e.g. `"jwt"` or `"api-key"`.
    fn name(&self) -> &'static str;

    /// The session cookie this scheme reads, when it is a cookie scheme.
    ///
    /// The host compares it with `CsrfConfig::session_cookie_name` and refuses
    /// to start on a mismatch: a drifted name means the CSRF middleware never
    /// sees the cookie and every cross-site request would pass unchecked.
    fn session_cookie_name(&self) -> Option<&str> {
        None
    }

    /// The server-side session repository this scheme validates cookies against.
    ///
    /// This is a **host hook**, not an application-facing port: the host uses it
    /// to re-check live WebSocket sessions after a cookie session is deleted.
    ///
    /// The host needs it to re-check a cookie-authenticated WebSocket session
    /// after the credential was issued: a deleted session must close the live
    /// connection (`WebSocketLimits::session_revalidation`). Non-cookie schemes
    /// return `None`; the default keeps custom schemes source-compatible.
    fn session_repository(
        &self,
    ) -> Option<std::sync::Arc<dyn crate::security::ISessionRepository>> {
        None
    }

    /// Whether this scheme recognizes the request's credential shape.
    ///
    /// Must be cheap and side-effect free — no I/O and no response mutation.
    fn matches(&self, ctx: &dyn IHttpContext) -> bool;

    /// Verify the credential and return claims.
    ///
    /// Return `Ok(None)` to continue anonymously; return
    /// `Err(Error::unauthorized(..))` to reject an explicitly presented but
    /// invalid credential.
    async fn authenticate(&self, ctx: &mut dyn IHttpContext) -> Result<Option<Box<dyn IClaims>>>;
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
/// and invokes them on every route that has `#[authorize]` — static and
/// `#[authorize(dynamic)]` alike. If no implementations are registered,
/// authorization is pass-through (no dynamic checks; a dynamic endpoint then
/// only requires an authenticated principal, so the application must enforce
/// its permissions elsewhere or register an authorizer).
///
/// # Methods
///
/// * `authorize_with_declared_rules` — receives the endpoint's
///   `&'static [AuthorizeRule]` slice directly (zero allocation, the same slice
///   as `RouteEntry.authorization`). Override this to see *which*
///   role/permission codes the endpoint declared; its default forwards to
///   `authorize`.
/// * `authorize` — receives the user's claims, matched route pattern, and HTTP
///   method. This is the rule-less fallback: the default
///   `authorize_with_declared_rules` ends up here, so an implementation that
///   only overrides `authorize` keeps working.
///
/// # Dynamic endpoints
///
/// On a route declared `#[authorize(dynamic)]` the static gate stops at
/// "authenticated" (no claims → 401) and the declaration in
/// `RouteEntry.authorization` is passed to the authorizers instead of being
/// evaluated against `claims.permissions()`. That makes an authorizer backed by
/// the database ("DB is the source of truth") authoritative per request, without
/// shipping a permission snapshot in the token.
#[async_trait::async_trait]
pub trait IDynamicAuthorizer: Send + Sync {
    /// Validate whether the authenticated user can access the resource.
    async fn authorize(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        method: &str,
    ) -> Result<()>;

    /// Validate access with the endpoint's declared `#[authorize]` rules.
    ///
    /// `declared` is the endpoint's `RouteEntry.authorization` slice itself,
    /// borrowed for the whole process. Every item is one `#[authorize]`
    /// attribute and all of them must hold (AND); inside one item the role and
    /// permission lists are any-of. On a `#[authorize(dynamic)]` endpoint the
    /// static gate does not evaluate these rules — this method decides.
    ///
    /// The default forwards to [`IDynamicAuthorizer::authorize`], so an
    /// implementation that only overrides `authorize` keeps working.
    async fn authorize_with_declared_rules(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        method: &str,
        _declared: &'static [crate::route::scan::AuthorizeRule],
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
    /// OAuth 2.0 client identifier (`client_id`, falling back to `azp`).
    ///
    /// Empty when the token carries neither claim. Additive: it identifies the
    /// client the credential was issued to (audit, per-client policy), it is not
    /// a permission source.
    #[serde(default)]
    pub client_id: String,
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
    ///
    /// The client identifier reads the `client_id` claim and falls back to
    /// `azp` (RFC 7519 "authorized party", as used by OIDC providers); an
    /// empty claim counts as absent.
    pub fn from_claims(claims: &dyn IClaims) -> Self {
        Self {
            id: claims.subject().to_string(),
            name: claims.get_username().unwrap_or_default().to_string(),
            email: claims.get_email().unwrap_or_default().to_string(),
            client_id: claims
                .claims()
                .get("client_id")
                .map(String::as_str)
                .filter(|client_id| !client_id.is_empty())
                .or_else(|| {
                    claims
                        .claims()
                        .get("azp")
                        .map(String::as_str)
                        .filter(|azp| !azp.is_empty())
                })
                .unwrap_or_default()
                .to_string(),
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

    /// The tenant identifier, or `Error::forbidden` when the principal carries
    /// no tenant claim.
    ///
    /// A missing tenant is **not** an authentication failure — the principal
    /// already proved who it is — it is a scope problem: the token simply does
    /// not reach any tenant. Reporting 403 instead of 401 keeps clients from
    /// retrying with the same valid credentials.
    pub fn require_tenant(&self) -> Result<&str> {
        self.tenant().ok_or_else(|| {
            Error::forbidden("the authenticated principal has no tenant claim".to_string())
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
            .ok_or_else(|| Error::unauthorized("authentication required".to_string()))
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
            Err(Error::forbidden(format!("{role} role required")))
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

    /// Require that the authenticated principal is the resource owner.
    ///
    /// * anonymous → `401 Unauthorized`
    /// * authenticated but `sub` differs from `owner_id` → `403 Forbidden`
    ///
    /// The 403 detail is deliberately generic (`resource owner required`) so it
    /// does not disclose the owner's id. Instance ownership stays in the handler
    /// (the framework cannot know a resource's shape); this helper centralizes
    /// only the 401/403 decision.
    fn require_owner(&self, owner_id: &str) -> Result<()> {
        let user = self.require_user()?;
        if user.id == owner_id {
            Ok(())
        } else {
            Err(Error::forbidden("resource owner required".to_string()))
        }
    }

    /// Require that a resource belongs to the caller's tenant.
    ///
    /// * anonymous → `401 Unauthorized`
    /// * authenticated without a tenant claim → `403 Forbidden`
    ///   (delegates to [`Self::require_tenant`])
    /// * a different tenant → `403 Forbidden` (`cross-tenant access is not
    ///   allowed`)
    ///
    /// `tenant_id` must come from the loaded resource, never from the request
    /// body/query — otherwise a caller could claim another tenant.
    fn require_same_tenant(&self, tenant_id: &str) -> Result<()> {
        let scope = self.require_tenant()?;
        if scope == tenant_id {
            Ok(())
        } else {
            Err(Error::forbidden(
                "cross-tenant access is not allowed".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            err.code() == "forbidden"
                && err
                    .public_detail()
                    .is_some_and(|detail| detail.contains("no tenant claim")),
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
            role_err.code() == "unauthorized"
                && role_err.public_detail() == Some("authentication required"),
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
            err.code() == "forbidden" && err.public_detail() == Some("auditor role required"),
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
        assert!(err.code() == "forbidden", "got {err:?}");
        assert_eq!(err.status_code(), 403);
    }

    // ── `authorize_with_declared_rules` default forwarding ────────────────

    const TWO_RULES: &[crate::route::scan::AuthorizeRule] = &[
        crate::route::scan::AuthorizeRule {
            roles: &["admin"],
            permissions: &[],
        },
        crate::route::scan::AuthorizeRule {
            roles: &[],
            permissions: &["order.create"],
        },
    ];

    struct TestClaims;

    impl IClaims for TestClaims {
        fn subject(&self) -> &str {
            "user-1"
        }
        fn roles(&self) -> &[String] {
            &[]
        }
        fn permissions(&self) -> &[String] {
            &[]
        }
        fn claims(&self) -> &HashMap<String, String> {
            static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
            EMPTY.get_or_init(HashMap::new)
        }
        fn clone_box(&self) -> Box<dyn IClaims> {
            Box::new(TestClaims)
        }
    }

    /// Implementation that overrides only the rule-less `authorize`.
    #[derive(Default)]
    struct AuthorizeOnly {
        calls: std::sync::Mutex<Vec<(String, String)>>,
    }

    #[async_trait::async_trait]
    impl IDynamicAuthorizer for AuthorizeOnly {
        async fn authorize(
            &self,
            _claims: &dyn IClaims,
            route_pattern: &str,
            method: &str,
        ) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push((route_pattern.to_string(), method.to_string()));
            Ok(())
        }
    }

    #[tokio::test]
    async fn declared_rules_default_forwards_to_authorize() {
        let authorizer = AuthorizeOnly::default();
        authorizer
            .authorize_with_declared_rules(&TestClaims, "/api/orders", "POST", TWO_RULES)
            .await
            .expect("the default must reach an `authorize`-only implementation");
        assert_eq!(
            authorizer.calls.lock().unwrap().as_slice(),
            &[("/api/orders".to_string(), "POST".to_string())],
            "the rules-aware default keeps an `authorize`-only implementation working"
        );
    }

    #[test]
    fn owner_and_tenant_helpers_split_401_from_403() {
        let admin = multi_tenant_admin();
        let request = TestRequest(Some(&admin));

        assert!(request.require_owner("user-1").is_ok());
        let err = request.require_owner("user-2").unwrap_err();
        assert_eq!(err.status_code(), 403);
        assert!(
            err.code() == "forbidden" && err.public_detail() == Some("resource owner required"),
            "{err:?}"
        );

        assert!(request.require_same_tenant("acme").is_ok());
        let err = request.require_same_tenant("other").unwrap_err();
        assert_eq!(err.status_code(), 403);
        assert!(
            err.code() == "forbidden"
                && err
                    .public_detail()
                    .is_some_and(|detail| detail.contains("cross-tenant")),
            "{err:?}"
        );

        // Authenticated but without a tenant claim: still 403, not 401.
        let single = single_tenant_user();
        let request = TestRequest(Some(&single));
        assert_eq!(
            request
                .require_same_tenant("acme")
                .unwrap_err()
                .status_code(),
            403
        );

        // Anonymous: authentication is the missing piece, so both are 401.
        let anonymous = TestRequest(None);
        assert_eq!(
            anonymous.require_owner("user-1").unwrap_err().status_code(),
            401
        );
        assert_eq!(
            anonymous
                .require_same_tenant("acme")
                .unwrap_err()
                .status_code(),
            401
        );
    }
}
