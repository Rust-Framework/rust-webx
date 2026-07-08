//! SaveChanges logging interceptor — audit trail without mutating entities.

use rust_ef::error::{EFError, EFResult};
use rust_ef::interceptor::{
    ISaveChangesInterceptor, SaveChangesContext, SaveChangesResultContext,
};

/// Logs pending change counts before and after each successful save.
pub struct SaveChangesLogInterceptor;

#[async_trait::async_trait]
impl ISaveChangesInterceptor for SaveChangesLogInterceptor {
    async fn on_saving(&self, ctx: &SaveChangesContext) -> EFResult<()> {
        tracing::debug!(
            added = ctx.added_count(),
            modified = ctx.modified_count(),
            deleted = ctx.deleted_count(),
            "SaveChanges starting"
        );
        Ok(())
    }

    async fn on_saved(
        &self,
        _ctx: &SaveChangesContext,
        result: &SaveChangesResultContext,
    ) -> EFResult<()> {
        tracing::debug!(
            added = result.added,
            updated = result.updated,
            deleted = result.deleted,
            "SaveChanges completed"
        );
        Ok(())
    }

    async fn on_save_failed(&self, _ctx: &SaveChangesContext, error: &EFError) {
        tracing::error!(error = %error, "SaveChanges failed");
    }
}
