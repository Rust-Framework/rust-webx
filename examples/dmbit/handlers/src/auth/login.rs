//! `POST /api/auth/login` — exchange credentials for a token.

use std::sync::Arc;

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::auth::{AuthResponse, LoginRequest, UserModel};

use super::queries::user_by_email;

#[derive(Inject)]
pub struct LoginHandler {
    #[inject(owned)]
    ctx: DbContext,
    /// Framework port: the application never signs a JWT itself.
    #[inject]
    tokens: Arc<dyn ITokenIssuer>,
    /// Framework port: the application never hashes a password itself.
    #[inject]
    hasher: Arc<dyn IPasswordHasher>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler {
    async fn handle(&mut self, req: LoginRequest) -> ActionResult<AuthResponse> {
        let user = user_by_email(&mut self.ctx, &req.email)
            .await?
            .ok_or_else(|| Error::bad_request("邮箱或密码错误"))?;

        if user.password_hash.is_empty()
            || !self
                .hasher
                .verify(&req.password, &user.password_hash)
                .await?
        {
            return Err(Error::bad_request("邮箱或密码错误"));
        }

        let model = UserModel::from(&user);
        let claims = TokenClaims::new(&model.id)
            .with_claim("name", model.name.clone())
            .with_claim("email", model.email.clone())
            .with_roles(model.roles.iter().cloned());
        let token = self.tokens.issue(claims).await?.token;

        tracing::info!("[Auth] login: {} ({})", model.email, model.id);
        Ok(AuthResponse { token, user: model })
    }
}
