//! Cache demo — MemoryCache get-or-create pattern.

use rust_webapp::*;

pub struct CacheStatsRequest;

#[get("/api/cache/stats")]
impl IRequest<String> for CacheStatsRequest {}
