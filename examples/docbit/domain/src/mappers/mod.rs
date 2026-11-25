//! Mappers — one file per context, expressed with std conversions plus the
//! framework's patch contract.
//!
//! | Direction | Contract |
//! |-----------|----------|
//! | create request → entity | [`TryFrom`] (validates required fields; `id`, timestamps, audit columns and identity come from the server, never from the body) |
//! | entity → read model | [`From`] for `&Entity`, so a response never clones the row twice |
//! | update request → loaded entity | [`IPatchable`] (only `Some` fields are copied) |
//!
//! `std::convert` is the guide, not a framework contract: the framework only
//! owns [`IPatchable`]. A caller mutates a loaded row with
//! `ctx.edit(row, |r| r.patch(req))` and then runs `detect_changes` +
//! `save_changes`.
//!
//! Navigation-derived fields (`category_name`, `author_name`, `roles`) require
//! the caller to have loaded the navigation with `linq!(... include ...)` first;
//! an unloaded navigation yields the empty default rather than a second query.

pub mod blog_mapper;
pub mod category_mapper;
pub mod comment_mapper;
pub mod exhibition_mapper;
pub mod rbac_mapper;
pub mod tracking_mapper;
pub mod user_mapper;

use std::time::{SystemTime, UNIX_EPOCH};

/// Current Unix seconds for server-assigned timestamps.
///
/// Handlers used to pass the clock into the mapper; the create conversion owns
/// its timestamps now, so the clock lives next to the conversions.
pub(crate) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
