//! Common services — auto-registered via `#[lrdi::inject_attr]`.
//!
//! Services annotated with `#[lrdi::inject_attr]` are collected at compile time
//! by `ServiceCollection::from_injected()` in `Host::build()`. No manual
//! `.register()` call is needed.

use lrwf::*;

/// Role-based authorizer — allows admin users to access admin routes.
///
/// Auto-registered as `dyn IDynamicAuthorizer` singleton via lrdi 0.2.5.
#[lrdi::inject_attr(singleton, as = dyn IDynamicAuthorizer)]
pub struct RoleAuthorizer;

#[async_trait]
impl IDynamicAuthorizer for RoleAuthorizer {
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
        Err(Error::Http(format!(
            "Forbidden: admin role required for '{}'",
            route_pattern
        )))
    }
}
