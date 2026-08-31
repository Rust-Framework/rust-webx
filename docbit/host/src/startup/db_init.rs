//! Database initialization hosted service.

use std::sync::Arc;

use rust_ef::db_context::DbContext;
use rust_webx::*;

use docbit_contracts::docs::IDocumentService;
use docbit_domain::configure_for_init;

use super::admin_user;

async fn ensure_schema(ctx: &mut DbContext) -> Result<()> {
    match ctx.ensure_created().await {
        Ok(()) => Ok(()),
        Err(e) if is_schema_mismatch(&e) => {
            tracing::warn!(
                "[DbInit] Existing database schema is incompatible ({}); recreating...",
                e
            );
            ctx.ensure_deleted()
                .await
                .map_err(|e| Error::Internal(format!("ensure_deleted failed: {}", e)))?;
            ctx.ensure_created()
                .await
                .map_err(|e| Error::Internal(format!("ensure_created failed: {}", e)))
        }
        Err(e) => Err(Error::Internal(format!("ensure_created failed: {}", e))),
    }
}

fn is_schema_mismatch(err: &dyn std::fmt::Display) -> bool {
    let msg = err.to_string().to_ascii_lowercase();
    msg.contains("datatype mismatch") || msg.contains("no such column")
}

#[derive(Inject)]
pub struct DbInitService {
    #[inject]
    docs: Arc<dyn IDocumentService>,
}

#[inject]
#[async_trait]
impl IHostedService for DbInitService {
    async fn start(&self) -> Result<()> {
        tracing::info!("[DbInit] Starting initialization...");

        let mut ctx: DbContext = dispatch_provider()
            .get_owned()
            .map_err(|e| Error::Internal(format!("DbContext resolution failed: {}", e)))?;

        configure_for_init(&mut ctx);

        ensure_schema(&mut ctx).await?;

        tracing::info!("[DbInit] Tables created and seed data applied.");

        admin_user::ensure_admin_user(&mut ctx).await?;

        self.docs
            .ensure_all_indexes()
            .map_err(|e| Error::Internal(format!("Doc index generation failed: {}", e)))?;

        let wwwroot = rust_webx::app_base().join("wwwroot");
        self.docs
            .sync_portfolio_assets(&wwwroot)
            .map_err(|e| Error::Internal(format!("Portfolio asset sync failed: {}", e)))?;

        tracing::info!("[DbInit] Initialization complete.");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        tracing::info!("[DbInit] Shutting down.");
        Ok(())
    }
}
