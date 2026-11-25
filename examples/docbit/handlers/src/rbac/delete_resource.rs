//! `DELETE /api/resources/{id}` — soft delete a resource (admin).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::rbac::DeleteResourceRequest;
use docbit_domain::entities::Resource;

use crate::common::{now_secs, operator_id, parse_id, save_changes};

#[derive(Inject)]
pub struct DeleteResourceHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteResourceRequest, String> for DeleteResourceHandler {
    async fn handle(&mut self, req: DeleteResourceRequest) -> ActionResult<String> {
        let id = parse_id(&req.id)?;
        let q = id.clone();

        let res = linq!(self.ctx.set::<Resource>(), |r: Resource| r.id == q)
            .first_or_default()
            .await?
            .ok_or_else(|| Error::not_found("Resource not found"))?;

        self.ctx.edit(res, |r| {
            r.is_deleted = true;
            r.updated_id = operator_id();
            r.updated_at = now_secs();
        });
        self.ctx.detect_changes();

        save_changes(&mut self.ctx).await?;

        Ok(format!("Deleted resource {}", id))
    }
}
