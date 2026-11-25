//! First-boot schema install + seed — runs once when the host starts.
//!
//! `bootstrap()` is the official history-free code-first install: on an **empty**
//! database it creates tables, indexes and foreign keys plus every `has_data`
//! seed row, and never writes `__ef_migrations_history`. It fails loudly on a
//! non-empty schema, which is the intended behaviour — an existing database is
//! upgraded with migrations (`ctx.migrate(..)`), never re-created behind the
//! operator's back. `ensure_created`/`ensure_deleted` stay test-only.
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

/// Install the schema on an empty database (or report why it cannot).
async fn ensure_schema(ctx: &mut DbContext) -> Result<()> {
    ctx.bootstrap()
        .await
        .map_err(|e| Error::di(format!("Database bootstrap failed: {e}")))
}

#[derive(Inject)]
pub struct DbInitService {
    #[inject]
    docs: Arc<dyn IDocumentService>,
    /// rust-dix built-in scope factory: background work opens its own scope
    /// instead of relying on a task-local ambient provider.
    #[inject]
    services: Arc<dyn ScopeFactory>,
    #[inject]
    hasher: Arc<dyn IPasswordHasher>,
}

#[inject]
#[async_trait]
impl IHostedService for DbInitService {
    async fn start(&self) -> Result<()> {
        tracing::info!("[DbInit] Starting initialization...");

        let scope = self.services.create_scope();
        let mut ctx: DbContext = scope
            .get_owned()
            .map_err(|e| Error::di(format!("DbContext resolution failed: {}", e)))?;

        configure_for_init(&mut ctx);

        ensure_schema(&mut ctx).await?;

        tracing::info!("[DbInit] Tables created and seed data applied.");

        admin_user::ensure_admin_user(&mut ctx, self.hasher.as_ref()).await?;

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
