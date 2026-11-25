//! `POST /api/auth/change-password` — rotate the current principal's password.

use std::sync::Arc;

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::auth::{ChangePasswordRequest, ChangePasswordResponse};

use super::queries::user_by_id;
use crate::common::{now_secs, operator_id, save_changes};

#[derive(Inject)]
pub struct ChangePasswordHandler {
    #[inject(owned)]
    ctx: DbContext,
    #[inject]
    hasher: Arc<dyn IPasswordHasher>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ChangePasswordRequest, ChangePasswordResponse> for ChangePasswordHandler {
    async fn handle(&mut self, req: ChangePasswordRequest) -> ActionResult<ChangePasswordResponse> {
        // The new password length is a declared field rule in the contracts
        // (`#[validate]`), so only the cross-field check is left here.
        let uid = req.require_user()?.id.clone();
        let row = user_by_id(&mut self.ctx, &uid)
            .await?
            .ok_or_else(|| Error::not_found("用户不存在"))?;

        if !self
            .hasher
            .verify(&req.old_password, &row.password_hash)
            .await?
        {
            return Err(Error::bad_request("原密码不正确"));
        }

        let hashed = self.hasher.hash(&req.new_password).await?;
        let now = now_secs();
        let actor = operator_id();
        self.ctx.edit(row, |user| {
            user.password_hash = hashed;
            user.updated_at = now;
            user.updated_id = actor;
        });

        self.ctx.detect_changes();
        save_changes(&mut self.ctx).await?;

        Ok(ChangePasswordResponse {
            message: "密码已更新".into(),
        })
    }
}
