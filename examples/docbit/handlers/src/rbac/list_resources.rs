//! `GET /api/resources` — every RBAC resource (admin).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::rbac::{ListResourcesRequest, ResourceModel};
use docbit_domain::entities::Resource;

use crate::common::MAX_LIST_ROWS;

#[derive(Inject)]
pub struct ListResourcesHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListResourcesRequest, Vec<ResourceModel>> for ListResourcesHandler {
    async fn handle(&mut self, _: ListResourcesRequest) -> ActionResult<Vec<ResourceModel>> {
        let items = linq!(self.ctx.set::<Resource>(); order_by r.name asc)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;

        Ok(items.iter().map(ResourceModel::from).collect())
    }
}
