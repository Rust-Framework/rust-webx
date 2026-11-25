//! `POST /api/role-users` — grant a role to a user (admin, idempotent).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::rbac::AssignRoleRequest;
use docbit_domain::entities::RoleUser;
use docbit_domain::new_id;

use crate::common::{now_secs, save_changes};

#[derive(Inject)]
pub struct AssignRoleHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<AssignRoleRequest, String> for AssignRoleHandler {
    async fn handle(&mut self, req: AssignRoleRequest) -> ActionResult<String> {
        let user_id = req.user_id.clone();
        let role_id = req.role_id.clone();

        // ── 校验：同一 (user, role) 只落一行 ──
        let (uid, rid) = (user_id.clone(), role_id.clone());
        let exists = linq!(self.ctx.set::<RoleUser>(), |r: RoleUser| r.user_id == uid
            && r.role_id == rid)
        .any()
        .await?;

        if exists {
            return Ok(format!(
                "Role {} already assigned to user {}",
                role_id, user_id
            ));
        }

        // ── 变更 ──
        self.ctx.add(RoleUser {
            id: new_id(),
            user_id: user_id.clone(),
            role_id: role_id.clone(),
            created_at: now_secs(),
        });

        save_changes(&mut self.ctx).await?;

        Ok(format!("Assigned role {} to user {}", role_id, user_id))
    }
}
