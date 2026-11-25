//! `GET /api/tracking` — the visit log, newest first (admin).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::tracking::{ListTrackingRequest, TrackingModel};
use docbit_domain::entities::Tracking;

use crate::common::MAX_LIST_ROWS;

#[derive(Inject)]
pub struct ListTrackingHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListTrackingRequest, Vec<TrackingModel>> for ListTrackingHandler {
    async fn handle(&mut self, _: ListTrackingRequest) -> ActionResult<Vec<TrackingModel>> {
        let items = linq!(self.ctx.set::<Tracking>(); order_by t.visited_at desc)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;

        Ok(items.iter().map(TrackingModel::from).collect())
    }
}
