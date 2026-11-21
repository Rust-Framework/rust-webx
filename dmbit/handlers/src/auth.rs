//! Auth handlers — login / me / change-password.

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use crate::db::EfResultExt;
use crate::util::{now_secs, operator_id};
use dmbit_contracts::auth::*;
use dmbit_domain::entities::User;

fn user_to_view(user: User) -> UserView {
    UserView {
        id: user.id,
        name: user.name,
        email: user.email,
        roles: user.roles.items().iter().map(|r| r.name.clone()).collect(),
        created_at: user.created_at,
    }
}

async fn create_token(tokens: &dyn ITokenIssuer, user: &UserView) -> Result<String> {
    let claims = TokenClaims::new(&user.id)
        .with_claim("name", user.name.clone())
        .with_claim("email", user.email.clone())
        .with_roles(user.roles.iter().cloned());
    Ok(tokens.issue(claims).await?.token)
}

async fn load_user_by_email(ctx: &mut DbContext, email: &str) -> Result<Option<User>> {
    let q = email.to_string();
    let users = linq!(ctx.set::<User>(), |u: User| u.email == q; include u.roles)
        .to_list()
        .await
        .map_ef()?;
    Ok(users.into_iter().next())
}

async fn load_user_by_id(ctx: &mut DbContext, id: &str) -> Result<Option<User>> {
    let q = id.to_string();
    let users = linq!(ctx.set::<User>(), |u: User| u.id == q; include u.roles)
        .to_list()
        .await
        .map_ef()?;
    Ok(users.into_iter().next())
}

#[derive(Inject)]
pub struct LoginHandler {
    #[inject(owned)]
    ctx: DbContext,
    #[inject]
    tokens: Arc<dyn ITokenIssuer>,
    #[inject]
    hasher: Arc<dyn IPasswordHasher>,
}

#[derive(Inject)]
pub struct AuthMeHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct ChangePasswordHandler {
    #[inject(owned)]
    ctx: DbContext,
    #[inject]
    hasher: Arc<dyn IPasswordHasher>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler {
    async fn handle(&mut self, req: LoginRequest) -> ActionResult<AuthResponse> {
        let user = load_user_by_email(&mut self.ctx, &req.email)
            .await?
            .ok_or_else(|| Error::Http("邮箱或密码错误".into()))?;

        if user.password_hash.is_empty()
            || !self
                .hasher
                .verify(&req.password, &user.password_hash)
                .await?
        {
            return Err(Error::Http("邮箱或密码错误".into()));
        }

        let model = user_to_view(user);
        let token = create_token(self.tokens.as_ref(), &model).await?;
        tracing::info!("[Auth] login: {} ({})", model.email, model.id);
        Ok(AuthResponse { token, user: model })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<AuthMeRequest, UserView> for AuthMeHandler {
    async fn handle(&mut self, _req: AuthMeRequest) -> ActionResult<UserView> {
        let uid = operator_id().ok_or_else(|| Error::Http("未登录".into()))?;
        let user = load_user_by_id(&mut self.ctx, &uid)
            .await?
            .ok_or_else(|| Error::Http("用户不存在".into()))?;
        Ok(user_to_view(user))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ChangePasswordRequest, ChangePasswordResponse> for ChangePasswordHandler {
    async fn handle(&mut self, req: ChangePasswordRequest) -> ActionResult<ChangePasswordResponse> {
        if req.new_password.len() < 6 {
            return Err(Error::Http("新密码至少 6 位".into()));
        }

        let uid = operator_id().ok_or_else(|| Error::Http("未登录".into()))?;
        let mut user = load_user_by_id(&mut self.ctx, &uid)
            .await?
            .ok_or_else(|| Error::Http("用户不存在".into()))?;

        if !self
            .hasher
            .verify(&req.old_password, &user.password_hash)
            .await?
        {
            return Err(Error::Http("原密码不正确".into()));
        }

        let hashed = self.hasher.hash(&req.new_password).await?;
        user.password_hash = hashed;
        user.updated_at = now_secs();
        user.updated_id = Some(uid);

        self.ctx.update(user);
        crate::db::save_changes(&mut self.ctx).await?;

        Ok(ChangePasswordResponse {
            message: "密码已更新".into(),
        })
    }
}
