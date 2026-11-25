//! `DELETE /api/role-users/{user_id}/{role_id}` — revoke a role (admin).
//!
//! `role_user` is a join table: it is hard-deleted, not soft-deleted.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::rbac::RevokeRoleRequest;
use docbit_domain::entities::RoleUser;

use crate::common::parse_id;

#[derive(Inject)]
pub struct RevokeRoleHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<RevokeRoleRequest, String> for RevokeRoleHandler {
    async fn handle(&mut self, req: RevokeRoleRequest) -> ActionResult<String> {
        let uid = parse_id(&req.user_id)?;
        let rid = parse_id(&req.role_id)?;
        let (uid_q, rid_q) = (uid.clone(), rid.clone());

        let affected = linq!(self.ctx.set::<RoleUser>(), |r: RoleUser| r.user_id == uid_q
            && r.role_id == rid_q)
        .execute_delete()
        .await?;

        if affected > 0 {
            Ok(format!("Revoked role {} from user {}", rid, uid))
        } else {
            Ok(format!("Role {} not assigned to user {}", rid, uid))
        }
    }
}
