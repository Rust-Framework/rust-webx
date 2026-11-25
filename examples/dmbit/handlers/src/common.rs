//! Shared application-layer support.
//!
//! Handlers reach for these helpers instead of repeating plumbing: one save
//! funnel, one clock, one id parser and the bound for the few endpoints that
//! intentionally keep a full-list contract. Loaded rows are mutated with
//! `DbContext::edit`, so the helper table no longer owns an attach path.

use std::time::{SystemTime, UNIX_EPOCH};

use rust_ef::db_context::DbContext;
use webx::{Error, RequestContext, Result};

/// Upper bound for a "full list" read.
///
/// Product tree / spec list / dashboard aggregate the whole fleet at once to
/// keep their existing non-paginated contract, so they read at most this many
/// rows instead of materialising an unbounded table. The device list, which
/// does grow with the fleet, is paginated with `PagedRequest`/`PagedResult`
/// instead of being silently truncated here.
pub const MAX_LIST_ROWS: usize = 2_000;

/// Persist pending changes, mapping ORM errors to framework errors at the edge.
pub async fn save_changes(ctx: &mut DbContext) -> Result<()> {
    ctx.save_changes().await?;
    Ok(())
}

/// Unix seconds — the timestamp unit stored by every entity.
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Trim a route-provided id and reject obviously invalid shapes before a query.
pub fn parse_id(s: &str) -> Result<String> {
    let t = s.trim();
    if t.is_empty() {
        return Err(Error::validation("标识不能为空"));
    }
    if t.chars().count() > 36 {
        return Err(Error::validation("标识格式无效"));
    }
    Ok(t.to_string())
}

/// The authenticated actor, for `created_id` / `updated_id` audit columns.
pub fn operator_id() -> Option<String> {
    RequestContext::operator_id()
}
