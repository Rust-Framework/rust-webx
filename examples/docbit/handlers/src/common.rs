//! Shared handler support — the crate's single copy of the persistence helpers,
//! list bounds, and request-scoped values.
//!
//! | Item | Role |
//! |------|------|
//! | [`save_changes`] | commit pending changes; `?` maps `EFError` → `webx::Error` |
//! | `ef_find_by_id!` / `ef_require_by_id!` | primary-key lookup macros |
//! | [`MAX_LIST_ROWS`] | row cap for every `to_list()` in this crate |
//! | [`now_secs`] / [`parse_id`] / [`operator_id`] | request-scoped values |
//! | [`uploads_root`] | writable root shared by media storage and upload staging |

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rust_ef::prelude::*;
use webx::{Error, RequestContext, Result};

/// Row cap for every list query in this crate.
///
/// Docbit's lists (portfolio works, categories, roles, users, docs index) are
/// admin-maintained catalogs rather than growing feeds, so they are **bounded**
/// instead of paginated: a runaway table must never turn one request into an
/// unbounded scan, and the SPA/e2e contract (`Vec<Model>`) stays stable.
pub const MAX_LIST_ROWS: usize = 200;

/// Persist pending changes, mapping ORM errors to framework errors.
pub async fn save_changes(ctx: &mut DbContext) -> Result<()> {
    ctx.save_changes().await?;
    Ok(())
}

/// Current Unix seconds.
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Validate a path parameter used as a string primary key.
pub fn parse_id(s: &str) -> Result<String> {
    if s.is_empty() {
        return Err(Error::validation(format!("Invalid id: {}", s)));
    }
    Ok(s.to_string())
}

/// Current request operator id (JWT `sub`), written by the HTTP dispatch pipeline.
pub fn operator_id() -> Option<String> {
    RequestContext::operator_id()
}

/// `uploads/` root — writable, beside the executable, deliberately outside the
/// embedded `wwwroot`. Shared by media storage and documentation staging.
pub fn uploads_root() -> PathBuf {
    webx::app_base().join("uploads")
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
