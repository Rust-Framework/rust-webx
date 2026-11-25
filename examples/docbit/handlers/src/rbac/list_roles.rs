//! `GET /api/roles` — every role (admin).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::rbac::{ListRolesRequest, RoleModel};
use docbit_domain::entities::Role;

use crate::common::MAX_LIST_ROWS;

#[derive(Inject)]
pub struct ListRolesHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListRolesRequest, Vec<RoleModel>> for ListRolesHandler {
    async fn handle(&mut self, _: ListRolesRequest) -> ActionResult<Vec<RoleModel>> {
        let roles = linq!(self.ctx.set::<Role>(); order_by r.name asc)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;

        Ok(roles.iter().map(RoleModel::from).collect())
    }
}
