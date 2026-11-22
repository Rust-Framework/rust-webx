//! DbContext helpers.
//!
//! `rust-ef` is Web-agnostic, so translating `EFError` into `webx::Error` is
//! the application-boundary decision owned by the shared `rust-webx-ef` crate.
//! This module re-exports that mapping and keeps the app-local helpers
//! (`save_changes`, id lookup macros); handler modules only call `.map_ef()?`.

use rust_ef::db_context::DbContext;
use webx::Result;

pub use webx_ef::{map_ef_error, EfResultExt};

/// Persist pending changes, mapping ORM errors to framework errors.
pub async fn save_changes(ctx: &mut DbContext) -> Result<()> {
    ctx.save_changes().await.map_ef()?;
    Ok(())
}

/// Find one row by String primary key.
#[macro_export]
macro_rules! ef_find_by_id {
    ($ctx:expr, $entity:ty, $id:expr $(; $($tail:tt)+)?) => {{
        use $crate::db::EfResultExt;
        let q = ($id).to_string();
        linq!(
            $ctx.set::<$entity>(),
            |row: $entity| row.id == q
            $(; $($tail)+)?
        )
        .first_or_default()
        .await
        .map_ef()
    }};
}

/// Find one row by id or return the given error.
#[macro_export]
macro_rules! ef_require_by_id {
    ($ctx:expr, $entity:ty, $id:expr, $err:expr $(; $($tail:tt)+)?) => {{
        $crate::ef_find_by_id!($ctx, $entity, $id $(; $($tail)+)?)?
            .ok_or_else(|| $err)?
    }};
}
