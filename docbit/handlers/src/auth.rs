//! Auth handlers — register / login / me / forgot-password / reset-password.
//!
//! JWT claims 含 `roles: Vec<String>` 支持多角色 RBAC。用户表 UUID 主键，
//! token 的 `sub` 存放 id 字符串。
//!
//! 每个 handler 持有 owned `DbContext`，`handle(&mut self, ...)` 直接操作 `self.ctx`。
//! 辅助函数 `load_user_by_email` / `load_user_by_id` 接收 `&mut DbContext`，
//! 因为 `DbContext::set::<T>()` 需要 `&mut self`。

use bcrypt::{hash, verify, DEFAULT_COST};
use jsonwebtoken::{encode, EncodingKey, Header};
use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;
use serde::{Deserialize, Serialize};

use docbit_contracts::auth::*;
use docbit_domain::entities::{PasswordResetToken, RoleUser, User};
use docbit_domain::{new_id, seed_ids};

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

/// 加载用户（含角色导航），按 email 过滤未删除记录。
async fn load_user_by_email(ctx: &mut DbContext, email: &str) -> Result<Option<User>> {
    let q = email.to_string();
    let users = linq!(ctx.set::<User>(), |u: User| u.email == q && !u.is_deleted; include u.roles)
        .to_list()
        .await
        .map_err(|e| Error::Internal(e.to_string()))?;

    Ok(users.into_iter().next())
}

/// 加载用户（含角色导航），按 id 过滤未删除记录。
async fn load_user_by_id(ctx: &mut DbContext, id: &str) -> Result<Option<User>> {
    let q = id.to_string();
    let users = linq!(ctx.set::<User>(), |u: User| u.id == q && !u.is_deleted; include u.roles)
        .to_list()
        .await
        .map_err(|e| Error::Internal(e.to_string()))?;

    Ok(users.into_iter().next())
}

#[derive(Inject)]
pub struct RegisterHandler {
    #[inject(owned)]
    ctx: DbContext,
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
pub struct ForgotPasswordHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct ResetPasswordHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<RegisterRequest, AuthResponse> for RegisterHandler {
    async fn handle(&mut self, req: RegisterRequest) -> Result<AuthResponse> {
        if load_user_by_email(&mut self.ctx, &req.email).await?.is_some() {
            return Err(Error::Http("Email already registered".into()));
        }

        let hashed = hash(&req.password, DEFAULT_COST)
            .map_err(|e| Error::Http(format!("Hash: {}", e)))?;
        let now = now_secs();
        let user_id = new_id();

        let user = User {
            id: user_id.clone(),
            name: req.name.clone(),
            email: req.email.clone(),
            password_hash: hashed,
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            roles: HasMany::new(),
        };
        let users = self.ctx.set::<User>();
        users.add(user);

        let role_user = RoleUser {
            id: new_id(),
            user_id: user_id.clone(),
            role_id: seed_ids::ROLE_USER.to_string(),
            created_at: now,
        };
        let role_users = self.ctx.set::<RoleUser>();
        role_users.add(role_user);

        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(format!("Failed to create user: {}", e)))?;

        let model = UserView {
            id: user_id,
            name: req.name,
            email: req.email,
            roles: vec!["user".into()],
            created_at: now,
        };
        let token = create_token(&model)?;

        tracing::info!("[Auth] User registered: {} ({})", model.name, model.id);
        Ok(AuthResponse { token, user: model })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler {
    async fn handle(&mut self, req: LoginRequest) -> Result<AuthResponse> {
        let user = load_user_by_email(&mut self.ctx, &req.email)
            .await?
            .ok_or_else(|| Error::Http("Invalid email or password".into()))?;

        if user.password_hash.is_empty()
            || !verify(&req.password, &user.password_hash)
                .map_err(|_| Error::Http("Authentication error".into()))?
        {
            return Err(Error::Http("Invalid email or password".into()));
        }

        let model = UserView {
            id: user.id,
            name: user.name.clone(),
            email: user.email.clone(),
            roles: user.roles.items().iter().map(|r| r.name.clone()).collect(),
            created_at: user.created_at,
        };
        let token = create_token(&model)?;

        tracing::info!("[Auth] User logged in: {} ({})", model.name, model.id);
        Ok(AuthResponse { token, user: model })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<AuthMeRequest, UserView> for AuthMeHandler {
    async fn handle(&mut self, req: AuthMeRequest) -> Result<UserView> {
        let uid = operator_id(req.claims.as_deref())
            .ok_or_else(|| Error::Http("Not authenticated".into()))?;

        let user = load_user_by_id(&mut self.ctx, &uid)
            .await?
            .ok_or_else(|| Error::Http("User not found".into()))?;

        Ok(UserView {
            id: user.id,
            name: user.name,
            email: user.email,
            roles: user.roles.items().iter().map(|r| r.name.clone()).collect(),
            created_at: user.created_at,
        })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ForgotPasswordRequest, ForgotPasswordResponse> for ForgotPasswordHandler {
    async fn handle(&mut self, req: ForgotPasswordRequest) -> Result<ForgotPasswordResponse> {
        let base_msg = "If the email exists, a reset link has been sent.".to_string();
        let Some(user) = load_user_by_email(&mut self.ctx, &req.email).await? else {
            return Ok(ForgotPasswordResponse {
                message: base_msg,
                reset_token: None,
            });
        };

        let token = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let expires = now_secs() + 3600;

        let token_record = PasswordResetToken {
            id: new_id(),
            token: token.clone(),
            user_id: user.id,
            expires_at: expires,
            used: 0,
        };
        let set = self.ctx.set::<PasswordResetToken>();
        set.add(token_record);

        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(format!("Failed to create reset token: {}", e)))?;

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

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ResetPasswordRequest, ResetPasswordResponse> for ResetPasswordHandler {
    async fn handle(&mut self, req: ResetPasswordRequest) -> Result<ResetPasswordResponse> {
        if req.password.len() < 6 {
            return Err(Error::Http("Password must be at least 6 characters".into()));
        }

        let q = req.token.clone();
        let mut record = linq!(self.ctx.set::<PasswordResetToken>(), |t: PasswordResetToken| t.token == q)
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::Http("Invalid or expired reset token".into()))?;

        if record.used != 0 {
            return Err(Error::Http("Reset token already used".into()));
        }
        if now_secs() > record.expires_at {
            return Err(Error::Http("Reset token expired".into()));
        }

        let hashed = hash(&req.password, DEFAULT_COST)
            .map_err(|e| Error::Http(format!("Hash: {}", e)))?;

        let user_id = record.user_id.clone();
        let mut user = load_user_by_id(&mut self.ctx, &user_id)
            .await?
            .ok_or_else(|| Error::Http("User not found".into()))?;

        user.password_hash = hashed;
        user.updated_at = now_secs();
        record.used = 1;

        let users = self.ctx.set::<User>();
        users.update(user);

        let tokens = self.ctx.set::<PasswordResetToken>();
        tokens.update(record);

        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        tracing::info!("[Auth] Password reset completed for user {}", user_id);
        Ok(ResetPasswordResponse {
            message: "Password updated successfully. You can now sign in.".into(),
        })
    }
}
