//! Common services — auto-registered via `#[lrdi::inject_attr]` and lref interceptors.
//!
//! Services annotated with `#[lrdi::inject_attr]` are collected at compile time
//! by `ServiceCollection::from_injected()` in `Host::build()`. No manual
//! `.register()` call is needed.

use lref::error::LrefResult;
use lref::interceptor::{ISaveChangesInterceptor, SaveChangesContext, SaveChangesResultContext};
use lref::prelude::*;
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

/// Audit interceptor — logs save operations via lref's `ISaveChangesInterceptor`.
///
/// Registered in `main.rs` via `DbContextOptionsBuilder::add_interceptor()`.
/// Logs before/after saves and on failures — analogous to EF Core's
/// `ISaveChangesInterceptor`.
pub(crate) struct AuditInterceptor;

#[async_trait::async_trait]
impl ISaveChangesInterceptor for AuditInterceptor {
    async fn on_saving(&self, ctx: &SaveChangesContext) -> LrefResult<()> {
        tracing::info!(
            "[Audit] Saving changes: {} added, {} modified, {} deleted ({} total entries)",
            ctx.added_count(),
            ctx.modified_count(),
            ctx.deleted_count(),
            ctx.total_count(),
        );
        Ok(())
    }

    async fn on_saved(
        &self,
        _ctx: &SaveChangesContext,
        result: &SaveChangesResultContext,
    ) -> LrefResult<()> {
        tracing::info!(
            "[Audit] Changes saved — {} entities persisted",
            result.total()
        );
        Ok(())
    }

    async fn on_save_failed(&self, _ctx: &SaveChangesContext, error: &LrefError) {
        tracing::warn!("[Audit] Save failed: {}", error);
    }
}
