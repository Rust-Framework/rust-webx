//! Audit interceptor — logs `save_changes()` operations via rust-ef's
//! `ISaveChangesInterceptor`.
//!
//! 注册方式：`DbContextOptionsBuilder::add_interceptor(AuditInterceptor)`。

use rust_ef::error::{EFError, EFResult};
use rust_ef::interceptor::{
    ISaveChangesInterceptor, SaveChangesContext, SaveChangesResultContext,
};

pub(crate) struct AuditInterceptor;

#[async_trait::async_trait]
impl ISaveChangesInterceptor for AuditInterceptor {
    async fn on_saving(&self, ctx: &SaveChangesContext) -> EFResult<()> {
        tracing::info!(
            "[Audit] Saving: {} added, {} modified, {} deleted ({} total)",
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
    ) -> EFResult<()> {
        tracing::info!("[Audit] Saved — {} entities persisted", result.total());
        Ok(())
    }

    async fn on_save_failed(&self, _ctx: &SaveChangesContext, error: &EFError) {
        tracing::warn!("[Audit] Save failed: {}", error);
    }
}
