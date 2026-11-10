//! Cache demo — MemoryCache get-or-create pattern.

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct CacheStatsRequest;

#[get("/api/cache/stats")]
impl IRequest<String> for CacheStatsRequest {}
