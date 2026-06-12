//! Cache demo — MemoryCache get-or-create pattern at `/api/cache/stats`.

use lrwf::*;
use std::sync::OnceLock;
use std::time::Duration;

fn shared_cache() -> &'static MemoryCache {
    static CACHE: OnceLock<MemoryCache> = OnceLock::new();
    CACHE.get_or_init(|| MemoryCache::new().with_max_entries(1000))
}

/// Request for cache statistics.
pub struct CacheStatsRequest;

#[get("/api/cache/stats")]
impl IRequest<String> for CacheStatsRequest {}

#[derive(Default)]
pub struct CacheStatsHandler;

#[handler]
#[async_trait]
impl IRequestHandler<CacheStatsRequest, String> for CacheStatsHandler {
    async fn handle(&self, _: CacheStatsRequest) -> Result<String> {
        let cache = shared_cache();
        let opts = DistributedCacheEntryOptions::new()
            .set_absolute_expiration_relative_to_now(Duration::from_secs(30));

        let val: String = cache
            .get_or_create("demo:status", || async { "cached".to_string() }, &opts)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        Ok(serde_json::json!({
            "status": val,
            "entries": cache.count().await,
        })
        .to_string())
    }
}
