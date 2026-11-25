//! `GET /api/exhibitions` — every portfolio work, in display order.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::exhibition::{ExhibitionModel, ListExhibitionsRequest};
use docbit_domain::entities::Exhibition;

use crate::common::MAX_LIST_ROWS;

#[derive(Inject)]
pub struct ListExhibitionsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListExhibitionsRequest, Vec<ExhibitionModel>> for ListExhibitionsHandler {
    async fn handle(&mut self, _: ListExhibitionsRequest) -> ActionResult<Vec<ExhibitionModel>> {
        let items =
            linq!(self.ctx.set::<Exhibition>(); include e.category; order_by e.sort_order asc)
                .take(MAX_LIST_ROWS)
                .to_list()
                .await?;

        Ok(items.iter().map(ExhibitionModel::from).collect())
    }
}
