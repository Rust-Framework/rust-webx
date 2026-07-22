//! Auth handlers — login / me / change-password.

use bcrypt::{hash, verify, DEFAULT_COST};
use jsonwebtoken::{encode, EncodingKey, Header};
use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;
use serde::{Deserialize, Serialize};

use dmbit_contracts::auth::*;
use dmbit_domain::entities::User;

use crate::db::EfResultExt;
use crate::util::{now_secs, operator_id};

#[derive(Debug, Serialize, Deserialize)]
struct AppJwtClaims {
    sub: String,
    name: String,
    email: String,
    #[serde(default)]
    roles: Vec<String>,
    iat: u64,
    exp: u64,
}

fn create_token(user: &UserView) -> Result<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    encode(
        &Header::default(),
        &AppJwtClaims {
            sub: user.id.clone(),
            name: user.name.clone(),
            email: user.email.clone(),
            roles: user.roles.clone(),
            iat: now,
            exp: now + 86_400,
        },
        &EncodingKey::from_secret(jwt_secret().as_bytes()),
    )
    .map_err(|e| Error::Http(format!("Token creation failed: {}", e)))
}

fn user_to_view(user: User) -> UserView {
    UserView {
        id: user.id,
        name: user.name,
        email: user.email,
        roles: user.roles.items().iter().map(|r| r.name.clone()).collect(),
        created_at: user.created_at,
    }
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
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler {
    async fn handle(&mut self, req: LoginRequest) -> Result<AuthResponse> {
        let user = load_user_by_email(&mut self.ctx, &req.email)
            .await?
            .ok_or_else(|| Error::Http("邮箱或密码错误".into()))?;

        if user.password_hash.is_empty()
            || !verify(&req.password, &user.password_hash)
                .map_err(|_| Error::Http("认证失败".into()))?
        {
            return Err(Error::Http("邮箱或密码错误".into()));
        }

        let model = user_to_view(user);
        let token = create_token(&model)?;
        tracing::info!("[Auth] login: {} ({})", model.email, model.id);
        Ok(AuthResponse { token, user: model })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<AuthMeRequest, UserView> for AuthMeHandler {
    async fn handle(&mut self, _req: AuthMeRequest) -> Result<UserView> {
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
    async fn handle(&mut self, req: ChangePasswordRequest) -> Result<ChangePasswordResponse> {
        if req.new_password.len() < 6 {
            return Err(Error::Http("新密码至少 6 位".into()));
        }

        let uid = operator_id().ok_or_else(|| Error::Http("未登录".into()))?;
        let mut user = load_user_by_id(&mut self.ctx, &uid)
            .await?
            .ok_or_else(|| Error::Http("用户不存在".into()))?;

        if !verify(&req.old_password, &user.password_hash)
            .map_err(|_| Error::Http("认证失败".into()))?
        {
            return Err(Error::Http("原密码不正确".into()));
        }

        let hashed = hash(&req.new_password, DEFAULT_COST)
            .map_err(|e| Error::Http(format!("Hash: {}", e)))?;
        user.password_hash = hashed;
        user.updated_at = now_secs();
        user.updated_id = Some(uid);

        let users = self.ctx.set::<User>();
        users.update(user);
        crate::db::save_changes(&mut self.ctx).await?;

        Ok(ChangePasswordResponse {
            message: "密码已更新".into(),
        })
    }
}
