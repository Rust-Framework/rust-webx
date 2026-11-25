//! Dynamic authorization — the endpoint's own `#[authorize]` declarations are
//! the source of truth.
//!
//! The host hands the matched endpoint's `&'static [AuthorizeRule]` slice to
//! [`IDynamicAuthorizer::authorize_with_declared_rules`], so the authorizer reads
//! *what the endpoint declared* instead of re-deriving a policy from the route
//! string. Semantics:
//!
//! * every declared rule must hold (AND across attributes);
//! * inside one rule the roles are any-of and the permissions are any-of, and
//!   the two dimensions are ANDed — [`AuthorizeRule::matches`] is that rule;
//! * an endpoint with a bare `#[authorize]` therefore means "any authenticated
//!   principal", which is exactly what the auth surface (`/api/auth/*`) wants.
//!
//! `authorize` stays as the rule-less fallback for endpoints without a
//! declaration; it shares the same route policy instead of growing a second
//! one.

use webx::*;

#[derive(Inject)]
pub struct RoleAuthorizer;

#[inject]
#[async_trait]
impl IDynamicAuthorizer for RoleAuthorizer {
    /// Primary path: judge the declarations, not the route string.
    async fn authorize_with_declared_rules(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        method: &str,
        declared: &'static [AuthorizeRule],
    ) -> Result<()> {
        if declared.is_empty() {
            // No `#[authorize]` on this endpoint: fall back to the route policy.
            return self.authorize(claims, route_pattern, method).await;
        }
        require_rules(declared, claims, route_pattern)
    }

    /// Rule-less fallback for endpoints without a declaration: the auth surface
    /// is open to authenticated principals, every other route needs `admin`.
    async fn authorize(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        _method: &str,
    ) -> Result<()> {
        if route_pattern.starts_with("/api/auth/") {
            return Ok(());
        }
        if claims.has_role("admin") {
            return Ok(());
        }
        Err(Error::forbidden(format!(
            "无权限访问「{route_pattern}」，需要管理员角色"
        )))
    }
}

/// Every rule must hold; one rule's roles and permissions are each any-of.
fn require_rules(rules: &[AuthorizeRule], claims: &dyn IClaims, route_pattern: &str) -> Result<()> {
    for rule in rules {
        if !rule.matches(claims.roles(), claims.permissions()) {
            return Err(Error::forbidden(format!(
                "无权限访问「{route_pattern}」：当前身份不满足端点声明的角色/权限"
            )));
        }
    }
    Ok(())
}
