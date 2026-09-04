//! Tracking handlers — visit summary & list (admin-only).
//!
//! Tracking 为日志表，无审计字段与软删除；直接查询即可。
//!
//! 每个 handler 持有 owned `DbContext`，`handle(&mut self, ...)` 直接操作 `self.ctx`。

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use docbit_contracts::tracking::{GetTrackingSummaryRequest, ListTrackingRequest, TrackingSummary};
use docbit_domain::entities::Tracking;

use crate::db::EfResultExt;

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
impl IRequestHandler<GetTrackingSummaryRequest, TrackingSummary> for GetTrackingSummaryHandler {
    async fn handle(&mut self, _: GetTrackingSummaryRequest) -> Result<TrackingSummary> {
        let total: i64 = linq!(self.ctx.set::<Tracking>(); count).await.map_ef()?;

        let all = self
            .ctx
            .set::<Tracking>()
            .query()
            .to_list()
            .await
            .map_ef()?;

        let unique = all
            .iter()
            .map(|t| t.path.as_str())
            .collect::<BTreeSet<_>>()
            .len() as i64;

        let today_start = today_start_secs();
        let today: i64 =
            linq!(self.ctx.set::<Tracking>(), |t: Tracking| t.visited_at >= today_start; count)
                .await
                .map_ef()?;

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
    async fn handle(
        &mut self,
        _: ListTrackingRequest,
    ) -> Result<Vec<docbit_contracts::tracking::TrackingModel>> {
        let items = linq!(self.ctx.set::<Tracking>(); order_by t.visited_at desc)
            .to_list()
            .await
            .map_ef()?;

        Ok(items.into_iter().map(Into::into).collect())
    }
}

/// 今日 00:00 的 Unix 秒（UTC 取整近似）。
fn today_start_secs() -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    (now / 86400) * 86400
}
