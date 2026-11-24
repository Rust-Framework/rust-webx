//! DbContext helpers — persistence error mapping, save, and reload macros.
//!
//! `rust-ef` 2.9.1 implements `From<EFError> for webx::Error`; this module keeps
//! the app-local helpers (`save_changes`, id lookup macros) and handler modules
//! only call `?`.

use rust_ef::db_context::DbContext;
use webx::Result;

/// Persist pending changes, mapping ORM errors to framework errors.
pub async fn save_changes(ctx: &mut DbContext) -> Result<()> {
    ctx.save_changes().await?;
    Ok(())
}

/// Find one row by String primary key.
#[macro_export]
macro_rules! ef_find_by_id {
    ($ctx:expr, $entity:ty, $id:expr $(; $($tail:tt)+)?) => {{

        let q = ($id).to_string();
        linq!(
            $ctx.set::<$entity>(),
            |row: $entity| row.id == q
            $(; $($tail)+)?
        )
        .first_or_default()
        .await

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
