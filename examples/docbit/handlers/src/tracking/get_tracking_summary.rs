//! `GET /api/tracking/summary` — visit counters (admin).
//!
//! `tracking` is an append-only log table with no audit fields and no soft
//! delete, so it is queried directly. Counters use SQL aggregates; only the
//! distinct-path count needs rows, and that read is bounded.

use std::collections::BTreeSet;

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::tracking::{GetTrackingSummaryRequest, TrackingSummary};
use docbit_domain::entities::Tracking;

use crate::common::MAX_LIST_ROWS;

#[derive(Inject)]
pub struct GetTrackingSummaryHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetTrackingSummaryRequest, TrackingSummary> for GetTrackingSummaryHandler {
    async fn handle(&mut self, _: GetTrackingSummaryRequest) -> ActionResult<TrackingSummary> {
        let total: i64 = linq!(self.ctx.set::<Tracking>(); count).await?;

        let today_start = today_start_secs();
        let today: i64 =
            linq!(self.ctx.set::<Tracking>(), |t: Tracking| t.visited_at >= today_start; count)
                .await?;

        // Distinct paths is not a supported aggregate here, so read a bounded
        // window of rows instead of the whole log.
        let recent = linq!(self.ctx.set::<Tracking>(); order_by t.visited_at desc)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;
        let unique = recent
            .iter()
            .map(|t| t.path.as_str())
            .collect::<BTreeSet<_>>()
            .len() as i64;

        Ok(TrackingSummary {
            total_visits: total,
            unique_paths: unique,
            today_visits: today,
        })
    }
}

/// 今日 00:00 的 Unix 秒（UTC 取整近似）。
fn today_start_secs() -> i64 {
    (crate::common::now_secs() / 86_400) * 86_400
}
