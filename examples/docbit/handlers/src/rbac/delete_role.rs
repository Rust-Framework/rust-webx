//! `DELETE /api/roles/{id}` — soft delete a role (admin).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::rbac::DeleteRoleRequest;
use docbit_domain::entities::Role;

use crate::common::{now_secs, operator_id, parse_id, save_changes};

#[derive(Inject)]
pub struct DeleteRoleHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteRoleRequest, String> for DeleteRoleHandler {
    async fn handle(&mut self, req: DeleteRoleRequest) -> ActionResult<String> {
        let id = parse_id(&req.id)?;
        let q = id.clone();

        let role = linq!(self.ctx.set::<Role>(), |r: Role| r.id == q)
            .first_or_default()
            .await?
            .ok_or_else(|| Error::not_found("Role not found"))?;

        self.ctx.edit(role, |r| {
            r.is_deleted = true;
            r.updated_id = operator_id();
            r.updated_at = now_secs();
        });
        self.ctx.detect_changes();

        save_changes(&mut self.ctx).await?;

        Ok(format!("Deleted role {}", id))
    }
}
