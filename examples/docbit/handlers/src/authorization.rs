//! Dynamic authorization — gates every `#[authorize]` route.
//!
//! Registered by `#[inject]` as a `dyn IDynamicAuthorizer` singleton, so the
//! host collects it while building and applies it to all authorized routes.
//!
//! Policy, in order:
//!
//! 1. `/api/auth/*` — the login/register flows themselves (public).
//! 2. `/api/comments*` — any signed-in user; comment ownership is checked in the
//!    handler, where the resource is actually loaded.
//! 3. Declared rules — every `#[authorize]` attribute on the endpoint must hold
//!    (AND), and inside one rule the role and permission lists are any-of. A
//!    rule with no constraints is the "any authenticated user" sentinel, which
//!    docbit treats as *undeclared*: it falls through to rule 4 so admin-authored
//!    endpoints keep needing `admin`.
//! 4. Everything else under `/api/*` — `admin`.

use webx::*;

#[derive(Inject)]
pub struct RoleAuthorizer;

impl RoleAuthorizer {
    /// The docbit policy used when an endpoint declares no constraining rule.
    fn policy_gate(claims: &dyn IClaims, route_pattern: &str) -> Result<()> {
        if claims.has_role("admin") {
            return Ok(());
        }
        Err(Error::forbidden(format!(
            "admin role required for '{}'",
            route_pattern
        )))
    }

    /// Public prefixes that never reach the policy gate.
    fn is_public(route_pattern: &str) -> bool {
        route_pattern.starts_with("/api/auth/") || route_pattern.contains("/comments")
    }
}

#[inject]
#[async_trait]
impl IDynamicAuthorizer for RoleAuthorizer {
    /// Primary entry point (1.3+): the endpoint's own `#[authorize]` declarations.
    async fn authorize_with_declared_rules(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        method: &str,
        rules: &'static [AuthorizeRule],
    ) -> Result<()> {
        if Self::is_public(route_pattern) {
            return Ok(());
        }

        let constraining = rules
            .iter()
            .filter(|rule| !rule.roles.is_empty() || !rule.permissions.is_empty())
            .collect::<Vec<_>>();

        if constraining.is_empty() {
            // Only unconstrained `#[authorize]` declarations (or none at all):
            // docbit's admin policy applies, exactly as before.
            return self.authorize(claims, route_pattern, method).await;
        }

        // Rules are ANDed; inside one rule roles/permissions are any-of.
        let allowed = constraining
            .iter()
            .all(|rule| rule.matches(claims.roles(), claims.permissions()));
        if allowed {
            return Ok(());
        }

        Err(Error::forbidden(format!(
            "declared authorization rules not satisfied for '{}'",
            route_pattern
        )))
    }

    /// Fallback entry point: no declared rules, so the policy gate decides.
    async fn authorize(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        _method: &str,
    ) -> Result<()> {
        if Self::is_public(route_pattern) {
            return Ok(());
        }
        Self::policy_gate(claims, route_pattern)
    }
}
