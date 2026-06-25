//! Application startup — database migrations and documentation index sync.

use std::sync::Arc;

use rust_ef::db_context::DbContext;
use rust_webapp::*;
use tokio::sync::Mutex;

use crate::common::bootstrap::AppPaths;
use crate::contracts::docs::IDocumentService;

/// Runs EF migrations and syncs filesystem documentation on host start.
#[rust_dicore::inject_attr(singleton, as = dyn IHostedService)]
pub struct DbInitService {
    ctx: Arc<Mutex<DbContext>>,
    docs: Arc<dyn IDocumentService>,
    paths: Arc<AppPaths>,
}

#[async_trait]
impl IHostedService for DbInitService {
    async fn start(&self) -> Result<()> {
        tracing::info!("[DbInitService] Starting initialization...");

        {
            let mut ctx = self.ctx.lock().await;
            crate::domain::migrations::m001_initial_20260611::up(&mut ctx)
                .await
                .map_err(|e| Error::Internal(format!("Migration failed: {}", e)))?;
            crate::domain::migrations::m002_blog_comments_auth::up(&mut ctx)
                .await
                .map_err(|e| Error::Internal(format!("Migration 002 failed: {}", e)))?;
            crate::domain::migrations::m003_drop_works_table::up(&mut ctx)
                .await
                .map_err(|e| Error::Internal(format!("Migration 003 failed: {}", e)))?;
            crate::domain::migrations::m004_drop_blog_posts_table::up(&mut ctx)
                .await
                .map_err(|e| Error::Internal(format!("Migration 004 failed: {}", e)))?;
        }

        tracing::info!("[DbInitService] Migrations applied.");

        self.docs
            .ensure_all_indexes()
            .map_err(|e| Error::Internal(format!("Doc index generation failed: {}", e)))?;

        self.docs
            .sync_portfolio_assets(&self.paths.wwwroot)
            .map_err(|e| Error::Internal(format!("Portfolio asset sync failed: {}", e)))?;

        tracing::info!("[DbInitService] Initialization complete.");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        tracing::info!("[DbInitService] Shutting down.");
        Ok(())
    }
}
