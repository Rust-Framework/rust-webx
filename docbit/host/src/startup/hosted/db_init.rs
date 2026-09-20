//! Schema ensure + first-boot seed — runs once when the host starts.
//!
//! Catalog synchronisation is delegated to `docbit_handlers::catalog`, the same
//! routine the documentation upload endpoint runs, so a boot and an upload can
//! never drift apart.

use std::sync::Arc;

use rust_ef::db_context::DbContext;
use webx::*;

use docbit_contracts::docs::IDocumentService;
use docbit_domain::configure_for_init;
use docbit_handlers::catalog;

use crate::startup::seed::admin_user;

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

        // Indexes → exhibition rows → logo copies. Same path the upload endpoint
        // takes, so a manual bundle upload and a restart converge.
        let wwwroot = webx::app_base().join("wwwroot");
        catalog::resync_catalog(&mut ctx, self.docs.as_ref(), &wwwroot).await?;

        tracing::info!("[DbInit] Initialization complete.");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        tracing::info!("[DbInit] Shutting down.");
        Ok(())
    }
}
