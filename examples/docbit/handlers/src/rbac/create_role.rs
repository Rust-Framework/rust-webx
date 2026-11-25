//! `POST /api/roles` — create a role (admin).

use rust_ef::db_context::DbContext;
use webx::*;

use docbit_contracts::rbac::{CreateRoleRequest, RoleModel};
use docbit_domain::entities::Role;

use crate::common::save_changes;

#[derive(Inject)]
pub struct CreateRoleHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateRoleRequest, RoleModel> for CreateRoleHandler {
    async fn handle(&mut self, req: CreateRoleRequest) -> ActionResult<RoleModel> {
        let entity = Role::try_from(req)?;
        let model = RoleModel::from(&entity);
        self.ctx.add(entity);

        save_changes(&mut self.ctx).await?;

        Ok(model)
    }
}
