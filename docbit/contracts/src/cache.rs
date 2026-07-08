//! Cache demo — MemoryCache get-or-create pattern.

use rust_webx::*;

#[derive(Default)]
pub struct CacheStatsRequest;

#[get("/api/cache/stats")]
impl IRequest<String> for CacheStatsRequest {}
