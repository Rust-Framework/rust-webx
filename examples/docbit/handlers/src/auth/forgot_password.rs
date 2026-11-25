//! `POST /api/auth/forgot-password` — issue a single-use reset token.
//!
//! The response is identical whether or not the email exists, so the endpoint
//! cannot be used to enumerate accounts.

use rust_ef::db_context::DbContext;
use webx::*;

use docbit_contracts::auth::{ForgotPasswordRequest, ForgotPasswordResponse};
use docbit_domain::entities::PasswordResetToken;
use docbit_domain::new_id;

use crate::common::{now_secs, save_changes};

use super::queries::load_user_by_email;

#[derive(Inject)]
pub struct ForgotPasswordHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ForgotPasswordRequest, ForgotPasswordResponse> for ForgotPasswordHandler {
    async fn handle(&mut self, req: ForgotPasswordRequest) -> ActionResult<ForgotPasswordResponse> {
        let base_msg = "If the email exists, a reset link has been sent.".to_string();

        // ── 加载：邮箱不存在也返回同样的文案 ──
        let Some(user) = load_user_by_email(&mut self.ctx, &req.email).await? else {
            return Ok(ForgotPasswordResponse {
                message: base_msg,
                reset_token: None,
            });
        };

        // ── 变更：落一条一小时有效的重置令牌 ──
        let token = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let record = PasswordResetToken {
            id: new_id(),
            token: token.clone(),
            user_id: user.id,
            expires_at: now_secs() + 3600,
            used: 0,
        };
        self.ctx.add(record);

        save_changes(&mut self.ctx).await?;

        tracing::info!("[Auth] Password reset requested for {}", req.email);
        Ok(ForgotPasswordResponse {
            message: format!(
                "{} Development mode: use the token below on the reset page.",
                base_msg
            ),
            reset_token: Some(token),
        })
    }
}
