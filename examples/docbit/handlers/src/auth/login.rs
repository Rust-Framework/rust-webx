//! `POST /api/auth/login` — verify the password hash and mint a JWT.

use std::sync::Arc;

use rust_ef::db_context::DbContext;
use webx::*;

use docbit_contracts::auth::{AuthResponse, LoginRequest, UserModel};

use super::queries::{create_token, load_user_by_email};

#[derive(Inject)]
pub struct LoginHandler {
    #[inject(owned)]
    ctx: DbContext,
    #[inject]
    tokens: Arc<dyn ITokenIssuer>,
    #[inject]
    hasher: Arc<dyn IPasswordHasher>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler {
    async fn handle(&mut self, req: LoginRequest) -> ActionResult<AuthResponse> {
        // ── 加载：按 email 取用户（含角色） ──
        let user = load_user_by_email(&mut self.ctx, &req.email)
            .await?
            .ok_or_else(|| Error::validation("Invalid email or password"))?;

        // ── 校验：统一用同一文案，避免枚举已注册邮箱 ──
        let verified = if user.password_hash.is_empty() {
            false
        } else {
            self.hasher
                .verify(&req.password, &user.password_hash)
                .await?
        };
        if !verified {
            return Err(Error::validation("Invalid email or password"));
        }

        // ── 响应 ──
        let model = UserModel::from(&user);
        let token = create_token(&*self.tokens, &model).await?;

        tracing::info!("[Auth] User logged in: {} ({})", model.name, model.id);
        Ok(AuthResponse { token, user: model })
    }
}
