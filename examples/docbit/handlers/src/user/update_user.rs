//! `PUT /api/users/{id}` — update name / email (admin).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::user::{UpdateUserRequest, UserModel};
use docbit_domain::entities::User;

use crate::common::{parse_id, save_changes};

#[derive(Inject)]
pub struct UpdateUserHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateUserRequest, UserModel> for UpdateUserHandler {
    async fn handle(&mut self, req: UpdateUserRequest) -> ActionResult<UserModel> {
        let id = parse_id(&req.id)?;
        let q = id.clone();

        // ── 加载 ──
        let user = linq!(self.ctx.set::<User>(), |u: User| u.id == q)
            .first_or_default()
            .await?
            .ok_or_else(|| Error::not_found("User not found"))?;

        // ── 变更与持久化 ──
        self.ctx.edit(user, |u| u.patch(req));
        self.ctx.detect_changes();

        save_changes(&mut self.ctx).await?;

        // ── 响应 ──
        let updated = crate::ef_require_by_id!(
            self.ctx,
            User,
            id,
            Error::not_found("User not found after update");
            include row.roles
        );

        Ok(UserModel::from(&updated))
    }
}
