//! Cache demo — MemoryCache get-or-create pattern.

use rust_webx::*;
use serde::Deserialize;

#[derive(Default, Deserialize)]
pub struct CacheStatsRequest;

#[get("/api/cache/stats")]
impl IRequest<String> for CacheStatsRequest {}
