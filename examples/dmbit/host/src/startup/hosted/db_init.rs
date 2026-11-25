//! Schema install on host start.
//!
//! Two paths, both official:
//!
//! * **empty database** → [`DbContext::bootstrap`]: create the model (tables,
//!   indexes, foreign keys) and insert the `has_data` seeds, without writing a
//!   migration history;
//! * **existing database** → runtime diff: introspect the live schema, diff it
//!   against the model and apply the additive upgrade. Removal-type changes are
//!   reported, never executed silently.
//!
//! `ensure_created` / `ensure_deleted` are test-only and are not used here.

use std::sync::Arc;

use rust_ef::db_context::DbContext;
use rust_ef::migration::MigrationEngine;
use webx::*;

use dmbit_domain::configure_for_init;

async fn ensure_schema(ctx: &mut DbContext) -> Result<()> {
    let model = ctx.model().build();
    let provider = ctx.provider();
    let dialect = provider.migration_dialect();
    let live = provider.introspect().await?;

    if live.is_empty() {
        tracing::info!("[DbInit] empty database — installing the model (bootstrap + seed)");
        ctx.bootstrap().await?;
        tracing::info!(
            "[DbInit] schema created; default admin seeded (inventory comes from CSV import)"
        );
        return Ok(());
    }

    let diff = MigrationEngine::new(dialect).diff_metas(&model, &live);
    if diff.is_empty() {
        tracing::info!("[DbInit] schema is up to date ({} tables)", live.len());
        return Ok(());
    }
    tracing::warn!(
        report = %diff.report(),
        "[DbInit] live schema drifted from the model — applying the additive upgrade"
    );
    MigrationEngine::new(dialect)
        .apply_diff(ctx.provider(), &diff)
        .await?;
    Ok(())
}

#[derive(Inject)]
pub struct DbInitService {
    /// rust-dix built-in scope factory: startup work opens its own scope.
    #[inject]
    services: Arc<dyn ScopeFactory>,
}

#[inject]
#[async_trait]
impl IHostedService for DbInitService {
    async fn start(&self) -> Result<()> {
        tracing::info!("[DbInit] Starting initialization...");

        let scope = self.services.create_scope();
        let mut ctx: DbContext = scope
            .get_owned()
            .map_err(|e| Error::internal(format!("DbContext resolution failed: {}", e)))?;

        configure_for_init(&mut ctx);
        ensure_schema(&mut ctx).await?;

        tracing::info!("[DbInit] Default admin: admin@dmbit.local / admin123");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        tracing::info!("[DbInit] Shutting down.");
        Ok(())
    }
}
