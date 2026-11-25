//! `POST /api/auth/reset-password` — redeem a reset token exactly once.

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::auth::{ResetPasswordRequest, ResetPasswordResponse};
use docbit_domain::entities::PasswordResetToken;

use crate::common::{now_secs, save_changes};

use super::queries::load_user_by_id;

#[derive(Inject)]
pub struct ResetPasswordHandler {
    #[inject(owned)]
    ctx: DbContext,
    #[inject]
    hasher: Arc<dyn IPasswordHasher>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ResetPasswordRequest, ResetPasswordResponse> for ResetPasswordHandler {
    async fn handle(&mut self, req: ResetPasswordRequest) -> ActionResult<ResetPasswordResponse> {
        // ── 校验 ──
        if req.password.len() < 6 {
            return Err(Error::validation("Password must be at least 6 characters"));
        }

        // ── 加载：令牌必须存在、未用过、未过期 ──
        let q = req.token.clone();
        let record = linq!(
            self.ctx.set::<PasswordResetToken>(),
            |t: PasswordResetToken| t.token == q
        )
        .first_or_default()
        .await?
        .ok_or_else(|| Error::validation("Invalid or expired reset token"))?;

        if record.used != 0 {
            return Err(Error::validation("Reset token already used"));
        }
        if now_secs() > record.expires_at {
            return Err(Error::validation("Reset token expired"));
        }

        // ── 变更：换新哈希，令牌置为已用 ──
        let hashed = self.hasher.hash(&req.password).await?;
        let user_id = record.user_id.clone();
        let user = load_user_by_id(&mut self.ctx, &user_id)
            .await?
            .ok_or_else(|| Error::not_found("User not found"))?;

        self.ctx.edit(user, |u| {
            u.password_hash = hashed;
            u.updated_at = now_secs();
        });
        self.ctx.edit(record, |r| r.used = 1);
        self.ctx.detect_changes();

        save_changes(&mut self.ctx).await?;

        tracing::info!("[Auth] Password reset completed for user {}", user_id);
        Ok(ResetPasswordResponse {
            message: "Password updated successfully. You can now sign in.".into(),
        })
    }
}
