//! Auth handlers — register / login / me / forgot-password / reset-password.
//!
//! JWT claims 含 `roles: Vec<String>` 支持多角色 RBAC。用户表 i32 自增主键，
//! token 的 `sub` 存放 id 字符串。

use std::sync::Arc;

use bcrypt::{hash, verify, DEFAULT_COST};
use jsonwebtoken::{encode, EncodingKey, Header};
use rust_ef::{
    db_context::DbContext,
    prelude::*,
};
use rust_webapp::*;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use docbit_contracts::auth::*;
use docbit_domain::entities::{PasswordResetToken, RoleUser, User};

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
            sub: user.id.to_string(),
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
async fn load_user_by_email(ctx: &Mutex<DbContext>, email: &str) -> Result<Option<User>> {
    let users = {
        let mut ctx = ctx.lock().await;
        linq!(ctx.set::<User>(), |u: User| u.email == email && !u.is_deleted; include u.roles)
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
    };
    Ok(users.into_iter().next())
}

/// 加载用户（含角色导航），按 id 过滤未删除记录。
async fn load_user_by_id(ctx: &Mutex<DbContext>, id: i32) -> Result<Option<User>> {
    let users = {
        let mut ctx = ctx.lock().await;
        linq!(ctx.set::<User>(), |u: User| u.id == id && !u.is_deleted; include u.roles)
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
    };
    Ok(users.into_iter().next())
}

#[derive(Inject)]
pub struct RegisterHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct LoginHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct AuthMeHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct ForgotPasswordHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct ResetPasswordHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[inject]
#[async_trait]
impl IRequestHandler<RegisterRequest, AuthResponse> for RegisterHandler {
    async fn handle(&self, req: RegisterRequest) -> Result<AuthResponse> {
        if load_user_by_email(&self.ctx, &req.email).await?.is_some() {
            return Err(Error::Http("Email already registered".into()));
        }

        let hashed = hash(&req.password, DEFAULT_COST)
            .map_err(|e| Error::Http(format!("Hash: {}", e)))?;
        let now = now_secs();
        let user = User {
            id: 0,
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
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<User>().add(user);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to create user: {}", e)))?;
        }

        // 自增主键在 save_changes 后清空跟踪器，按 email 回查以拿到 id
        let created = load_user_by_email(&self.ctx, &req.email)
            .await?
            .ok_or_else(|| Error::Internal("User disappeared after insert".into()))?;

        // 分配默认 "user" 角色（role_users 表 id=2）
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<RoleUser>().add(RoleUser {
                id: 0,
                user_id: created.id,
                role_id: 2,
                created_at: now,
            });
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to assign role: {}", e)))?;
        }

        let model = UserView {
            id: created.id,
            name: created.name.clone(),
            email: created.email.clone(),
            roles: created.roles.items().iter().map(|r| r.name.clone()).collect(),
            created_at: created.created_at,
        };
        let token = create_token(&model)?;
        tracing::info!("[Auth] User registered: {} ({})", model.name, model.id);
        Ok(AuthResponse { token, user: model })
    }
}

#[inject]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler {
    async fn handle(&self, req: LoginRequest) -> Result<AuthResponse> {
        let user = load_user_by_email(&self.ctx, &req.email)
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

#[inject]
#[async_trait]
impl IRequestHandler<AuthMeRequest, UserView> for AuthMeHandler {
    async fn handle(&self, req: AuthMeRequest) -> Result<UserView> {
        let uid = operator_id(req.claims.as_deref())
            .ok_or_else(|| Error::Http("Not authenticated".into()))?;
        let user = load_user_by_id(&self.ctx, uid)
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

#[inject]
#[async_trait]
impl IRequestHandler<ForgotPasswordRequest, ForgotPasswordResponse> for ForgotPasswordHandler {
    async fn handle(&self, req: ForgotPasswordRequest) -> Result<ForgotPasswordResponse> {
        let user = load_user_by_email(&self.ctx, &req.email).await?;
        let base_msg = "If the email exists, a reset link has been sent.".to_string();
        let Some(user) = user else {
            return Ok(ForgotPasswordResponse {
                message: base_msg,
                reset_token: None,
            });
        };

        // 生成 64 位 hex token
        let token = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let expires = now_secs() + 3600;
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<PasswordResetToken>().add(PasswordResetToken {
                id: 0,
                token: token.clone(),
                user_id: user.id,
                expires_at: expires,
                used: 0,
            });
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to create reset token: {}", e)))?;
        }

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

#[inject]
#[async_trait]
impl IRequestHandler<ResetPasswordRequest, ResetPasswordResponse> for ResetPasswordHandler {
    async fn handle(&self, req: ResetPasswordRequest) -> Result<ResetPasswordResponse> {
        if req.password.len() < 6 {
            return Err(Error::Http("Password must be at least 6 characters".into()));
        }

        let token = req.token.clone();
        let record = {
            let mut ctx = self.ctx.lock().await;
            let q = token.clone();
            linq!(ctx.set::<PasswordResetToken>(), |t: PasswordResetToken| t.token == q)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::Http("Invalid or expired reset token".into()))?;

        if record.used != 0 {
            return Err(Error::Http("Reset token already used".into()));
        }
        if now_secs() > record.expires_at {
            return Err(Error::Http("Reset token expired".into()));
        }

        let hashed = hash(&req.password, DEFAULT_COST)
            .map_err(|e| Error::Http(format!("Hash: {}", e)))?;

        let mut user = load_user_by_id(&self.ctx, record.user_id)
            .await?
            .ok_or_else(|| Error::Http("User not found".into()))?;
        user.password_hash = hashed;
        user.updated_at = now_secs();
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<User>().update(user);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }

        // 标记 token 已使用：先查回再更新（跟踪器在 save 后已清空）
        let mut used_record = {
            let mut ctx = self.ctx.lock().await;
            let q = token.clone();
            linq!(ctx.set::<PasswordResetToken>(), |t: PasswordResetToken| t.token == q)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::Http("Token vanished".into()))?;
        used_record.used = 1;
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<PasswordResetToken>().update(used_record);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }

        tracing::info!("[Auth] Password reset completed for user {}", record.user_id);
        Ok(ResetPasswordResponse {
            message: "Password updated successfully. You can now sign in.".into(),
        })
    }
}
