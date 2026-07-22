//! DbContext helpers.

use rust_ef::db_context::DbContext;
use rust_ef::error::EFError;
use rust_webx::{Error, Result};

pub fn map_ef_error(err: EFError) -> Error {
    match err {
        EFError::NotFound(msg, _) => Error::NotFound(msg),
        EFError::ConcurrencyConflict(msg, _) => Error::Conflict(msg),
        EFError::ModelValidation(msg, _) => Error::Validation(msg),
        EFError::Query(msg, _) => Error::Http(msg),
        other => Error::Internal(other.to_string()),
    }
}

pub trait EfResultExt<T> {
    fn map_ef(self) -> Result<T>;
}

impl<T> EfResultExt<T> for std::result::Result<T, EFError> {
    fn map_ef(self) -> Result<T> {
        self.map_err(map_ef_error)
    }
}

pub async fn save_changes(ctx: &mut DbContext) -> Result<()> {
    ctx.save_changes().await.map_ef()?;
    Ok(())
}

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

#[macro_export]
macro_rules! ef_require_by_id {
    ($ctx:expr, $entity:ty, $id:expr, $err:expr $(; $($tail:tt)+)?) => {{
        $crate::ef_find_by_id!($ctx, $entity, $id $(; $($tail)+)?)?
            .ok_or_else(|| $err)?
    }};
}
