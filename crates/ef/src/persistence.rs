//! DbContext `save_changes` with framework error mapping.

use rust_ef::db_context::DbContext;
use rust_webx_core::Result;

use crate::error::EfResultExt;

/// Persist pending changes, mapping ORM errors to HTTP-oriented framework errors.
pub async fn save_changes(ctx: &mut DbContext) -> Result<()> {
    ctx.save_changes().await.map_ef()?;
    Ok(())
}
