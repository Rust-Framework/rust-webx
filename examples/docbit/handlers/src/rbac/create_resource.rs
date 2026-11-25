//! `POST /api/resources` — create an RBAC resource (admin).

use rust_ef::db_context::DbContext;
use webx::*;

use docbit_contracts::rbac::{CreateResourceRequest, ResourceModel};
use docbit_domain::entities::Resource;

use crate::common::save_changes;

#[derive(Inject)]
pub struct CreateResourceHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateResourceRequest, ResourceModel> for CreateResourceHandler {
    async fn handle(&mut self, req: CreateResourceRequest) -> ActionResult<ResourceModel> {
        let entity = Resource::try_from(req)?;
        let model = ResourceModel::from(&entity);
        self.ctx.add(entity);

        save_changes(&mut self.ctx).await?;

        Ok(model)
    }
}
