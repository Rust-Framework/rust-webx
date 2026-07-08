//! Cache demo handler — illustrates framework `MemoryCache` usage.
//!
//! 演示 get-or-create 模式：从共享 `MemoryCache` 取 `demo:status`，
//! 若不存在则写入“cached”并设置 30 秒绝对过期。

use std::sync::Arc;
use std::time::Duration;

use rust_webx::*;

use docbit_contracts::cache::CacheStatsRequest;

#[derive(Inject)]
pub struct CacheStatsHandler {
    #[inject]
    cache: Arc<MemoryCache>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CacheStatsRequest, String> for CacheStatsHandler {
    async fn handle(&mut self, _: CacheStatsRequest) -> Result<String> {
        let opts = DistributedCacheEntryOptions::new()
            .set_absolute_expiration_relative_to_now(Duration::from_secs(30));

        let val: String = self
            .cache
            .get_or_create("demo:status", || async { "cached".to_string() }, &opts)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        let entries = self.cache.count().await;

        Ok(serde_json::json!({
            "status": val,
            "entries": entries,
        })
        .to_string())
    }
}
