//! `PUT /api/resources/{id}` — update an RBAC resource (admin).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::rbac::{ResourceModel, UpdateResourceRequest};
use docbit_domain::entities::Resource;

use crate::common::{parse_id, save_changes};

#[derive(Inject)]
pub struct UpdateResourceHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateResourceRequest, ResourceModel> for UpdateResourceHandler {
    async fn handle(&mut self, req: UpdateResourceRequest) -> ActionResult<ResourceModel> {
        let id = parse_id(&req.id)?;
        let q = id.clone();

        let res = linq!(self.ctx.set::<Resource>(), |r: Resource| r.id == q)
            .first_or_default()
            .await?
            .ok_or_else(|| Error::not_found("Resource not found"))?;

        let updated = {
            let mut updated = None;
            self.ctx.edit(res, |r| {
                r.patch(req);
                updated = Some(r.clone());
            });
            updated.ok_or_else(|| Error::internal("Resource patch did not run"))?
        };
        self.ctx.detect_changes();

        save_changes(&mut self.ctx).await?;

        Ok(ResourceModel::from(&updated))
    }
}
