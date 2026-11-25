//! `PUT /api/roles/{id}` — update a role (admin).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::rbac::{RoleModel, UpdateRoleRequest};
use docbit_domain::entities::Role;

use crate::common::{parse_id, save_changes};

#[derive(Inject)]
pub struct UpdateRoleHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateRoleRequest, RoleModel> for UpdateRoleHandler {
    async fn handle(&mut self, req: UpdateRoleRequest) -> ActionResult<RoleModel> {
        let id = parse_id(&req.id)?;
        let q = id.clone();

        let role = linq!(self.ctx.set::<Role>(), |r: Role| r.id == q)
            .first_or_default()
            .await?
            .ok_or_else(|| Error::not_found("Role not found"))?;

        let updated = {
            let mut updated = None;
            self.ctx.edit(role, |r| {
                r.patch(req);
                updated = Some(r.clone());
            });
            updated.ok_or_else(|| Error::internal("Role patch did not run"))?
        };
        self.ctx.detect_changes();

        save_changes(&mut self.ctx).await?;

        Ok(RoleModel::from(&updated))
    }
}
