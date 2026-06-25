//! Migration 003 — portfolio works are filesystem-driven via INDEX.json.

use rust_ef::db_context::{DbContext, IDbContext};

pub async fn up(ctx: &mut DbContext) -> Result<(), String> {
    ctx.provider()
        .execute_migration_command("DROP TABLE IF EXISTS works")
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!("[Migration] 003_drop_works_table: removed legacy works table");
    Ok(())
}
