//! Common services — includes the dynamic authorizer and auto-registration.
//!
//! To use, call `register_common_services` in the `register()` closure:
//!
//! ```ignore
//! .register(|svc| common::register_common_services(svc))
//! ```

use lrwf::*;
use std::sync::Arc;

use lrdi::ServiceCollection;

/// Role-based authorizer — allows admin users to access admin routes.
#[derive(Default)]
pub struct RoleAuthorizer;

#[async_trait]
impl IDynamicAuthorizer for RoleAuthorizer {
    async fn authorize(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        _method: &str,
    ) -> Result<()> {
        // Routes under /api/auth/ are accessible by any authenticated user
        if route_pattern.starts_with("/api/auth/") {
            return Ok(());
        }

        // All other protected routes require admin role
        if claims.has_role("admin") {
            return Ok(());
        }

        Err(Error::Http(format!(
            "Forbidden: admin role required for '{}'",
            route_pattern
        )))
    }
}

/// Auto-registration helper. Call inside `Host::builder().register(...)`:
///
/// ```ignore
/// .register(common::register_common_services)
/// ```
pub fn register_common_services(svc: ServiceCollection) -> ServiceCollection {
    svc.singleton::<dyn IDynamicAuthorizer>(|_| Arc::new(RoleAuthorizer::default()))
}
