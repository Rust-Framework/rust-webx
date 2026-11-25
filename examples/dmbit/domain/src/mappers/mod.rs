//! Mappers — one file per context, expressed with std conversions plus the
//! framework's patch contract.
//!
//! | Direction | Contract |
//! |-----------|----------|
//! | create request → entity | [`TryFrom`] (validates the request fields; `id`, timestamps and audit columns are server-assigned) |
//! | entity → read model | [`From`] for `&Entity` |
//! | update request → loaded entity | [`IPatchable`] (only `Some` fields are overwritten) |
//!
//! `std::convert` is the guide, not a framework contract: the framework only
//! owns [`IPatchable`]. Update requests carry raw text, so each write mapper
//! also exposes a `normalize_update_*` function that validates and trims the
//! patch before `ctx.edit(row, |r| r.patch(patch))` applies it; the patch itself
//! stays infallible. Non-request construction paths (device generation, CSV
//! import) keep their `Seed` struct with a plain `From<Seed> for Entity`.
//!
//! Navigation-derived fields require the caller to load the navigation first;
//! an unloaded navigation yields the empty default rather than a second query.

pub mod component_mapper;
pub mod device_mapper;
pub mod inventory_mapper;
pub mod product_mapper;
pub mod spec_mapper;
pub mod user_mapper;

use std::time::{SystemTime, UNIX_EPOCH};

/// Current Unix seconds for server-assigned timestamps.
pub(crate) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Current request operator id (JWT `sub`), set by the dispatch pipeline.
pub(crate) fn operator_id() -> Option<String> {
    webx_core::RequestContext::operator_id()
}
