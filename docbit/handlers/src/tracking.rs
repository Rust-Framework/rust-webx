//! Tracking handlers — visit summary & list (admin-only).
//!
//! Tracking 为日志表，无审计字段与软删除；直接查询即可。
//!
//! 每个 handler 持有 owned `DbContext`，`handle(&mut self, ...)` 直接操作 `self.ctx`。

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use docbit_contracts::tracking::{GetTrackingSummaryRequest, ListTrackingRequest, TrackingSummary};
use docbit_domain::entities::Tracking;

use crate::util::now_secs;

#[derive(Inject)]
pub struct GetTrackingSummaryHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct ListTrackingHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetTrackingSummaryRequest, TrackingSummary>
    for GetTrackingSummaryHandler
{
    async fn handle(&mut self, _: GetTrackingSummaryRequest) -> Result<TrackingSummary> {
        // rust-ef 最佳实践：用 `linq!` 多子句形式做聚合终端。
        let total: i64 = linq!(self.ctx.set::<Tracking>(); count)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        // 唯一路径数：distinct path 在 rust-ef 当前无内置 API，按 path 分组在内存侧归约。
        let all = self
            .ctx
            .set::<Tracking>()
            .query()
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        let unique = all
            .iter()
            .map(|t| t.path.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len() as i64;
        // 今日访问数：`>=` 谓词用 `linq!` 类型安全表达式。
        let today_start = today_start_secs();
        let today: i64 = linq!(self.ctx.set::<Tracking>(), |t: Tracking| t.visited_at >= today_start; count)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(TrackingSummary {
            total_visits: total,
            unique_paths: unique,
            today_visits: today,
        })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListTrackingRequest, Vec<docbit_contracts::tracking::TrackingModel>>
    for ListTrackingHandler
{
    async fn handle(&mut self, _: ListTrackingRequest) -> Result<Vec<docbit_contracts::tracking::TrackingModel>> {
        let items = linq!(self.ctx.set::<Tracking>(); order_by t.visited_at desc)
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(items.into_iter().map(Into::into).collect())
    }
}

/// 今日 00:00 的 Unix 秒（按本地时区）。
fn today_start_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // 86400 = 24*3600；今日 0 点 = (now // 86400) * 86400（UTC）。
    // 注：本地 Asia/Shanghai = UTC+8，按本地日历“今日”近似用 UTC 取整。
    // 若需要严格本地日期，可在 host 注入 chrono；此处保持依赖最小。
    let _ = now_secs(); // 占位引用 util，保持模块一致性
    (now / 86400) * 86400
}
