//! `DELETE /api/users/{id}` — soft delete (admin).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::user::DeleteUserRequest;
use docbit_domain::entities::User;

use crate::common::{now_secs, operator_id, parse_id, save_changes};

#[derive(Inject)]
pub struct DeleteUserHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteUserRequest, String> for DeleteUserHandler {
    async fn handle(&mut self, req: DeleteUserRequest) -> ActionResult<String> {
        let id = parse_id(&req.id)?;
        let q = id.clone();

        let user = linq!(self.ctx.set::<User>(), |u: User| u.id == q)
            .first_or_default()
            .await?
            .ok_or_else(|| Error::not_found("User not found"))?;

        self.ctx.edit(user, |u| {
            u.is_deleted = true;
            u.updated_id = operator_id();
            u.updated_at = now_secs();
        });
        self.ctx.detect_changes();

        save_changes(&mut self.ctx).await?;

        tracing::info!("[User] Soft-deleted: {}", id);
        Ok(format!("Deleted user {}", id))
    }
}
