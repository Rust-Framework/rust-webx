//! Auth handlers — register / login / me / forgot-password / reset-password.
//!
//! JWT claims 含 `roles: Vec<String>` 支持多角色 RBAC。用户表 UUID 主键，
//! token 的 `sub` 存放 id 字符串。
//!
//! 每个 handler 持有 owned `DbContext`，`handle(&mut self, ...)` 直接操作 `self.ctx`。
//! 辅助函数 `load_user_by_email` / `load_user_by_id` 接收 `&mut DbContext`，
//! 因为 `DbContext::set::<T>()` 需要 `&mut self`。

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::auth::*;
use docbit_domain::entities::{PasswordResetToken, RoleUser, User};
use docbit_domain::{new_id, seed_ids};

use crate::db::{save_changes, EfResultExt};
use crate::util::{now_secs, operator_id};

/// Mint a token through the framework port; the host stamps `iss`/`aud`
/// (when configured), `jti`, `iat` and `exp`.
async fn create_token(issuer: &dyn ITokenIssuer, user: &UserView) -> Result<String> {
    let issued = issuer
        .issue(
            TokenClaims::new(&user.id)
                .with_roles(user.roles.clone())
                .with_claim("name", user.name.clone())
                .with_claim("email", user.email.clone())
                .expires_in(std::time::Duration::from_secs(86_400)),
        )
        .await?;
    Ok(issued.token)
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

/// 加载用户（含角色导航），按 email 查询；全局过滤器排除已删除用户。
async fn load_user_by_email(ctx: &mut DbContext, email: &str) -> Result<Option<User>> {
    let q = email.to_string();
    let users = linq!(ctx.set::<User>(), |u: User| u.email == q; include u.roles)
        .to_list()
        .await
        .map_ef()?;

    Ok(users.into_iter().next())
}

/// 加载用户（含角色导航），按 id 查询；全局过滤器排除已删除用户。
async fn load_user_by_id(ctx: &mut DbContext, id: &str) -> Result<Option<User>> {
    let q = id.to_string();
    let users = linq!(ctx.set::<User>(), |u: User| u.id == q; include u.roles)
        .to_list()
        .await
        .map_ef()?;

    Ok(users.into_iter().next())
}

#[derive(Inject)]
pub struct RegisterHandler {
    #[inject(owned)]
    ctx: DbContext,
    #[inject]
    tokens: Arc<dyn ITokenIssuer>,
    #[inject]
    hasher: Arc<dyn IPasswordHasher>,
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
pub struct ForgotPasswordHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct ResetPasswordHandler {
    #[inject(owned)]
    ctx: DbContext,
    #[inject]
    hasher: Arc<dyn IPasswordHasher>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<RegisterRequest, AuthResponse> for RegisterHandler {
    async fn handle(&mut self, req: RegisterRequest) -> ActionResult<AuthResponse> {
        if load_user_by_email(&mut self.ctx, &req.email)
            .await?
            .is_some()
        {
            return Err(Error::bad_request("Email already registered"));
        }

        let hashed = self.hasher.hash(&req.password).await?;
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
        self.ctx.add(user);

        let role_user = RoleUser {
            id: new_id(),
            user_id: user_id.clone(),
            role_id: seed_ids::ROLE_USER.to_string(),
            created_at: now,
        };
        self.ctx.add(role_user);

        save_changes(&mut self.ctx).await?;

        let saved = crate::ef_require_by_id!(
            self.ctx,
            User,
            user_id,
            Error::not_found("User not found after register");
            include row.roles
        );
        let model = user_to_view(saved);
        let token = create_token(&*self.tokens, &model).await?;

        tracing::info!("[Auth] User registered: {} ({})", model.name, model.id);
        Ok(AuthResponse { token, user: model })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler {
    async fn handle(&mut self, req: LoginRequest) -> ActionResult<AuthResponse> {
        let user = load_user_by_email(&mut self.ctx, &req.email)
            .await?
            .ok_or_else(|| Error::bad_request("Invalid email or password"))?;

        let verified = if user.password_hash.is_empty() {
            false
        } else {
            self.hasher
                .verify(&req.password, &user.password_hash)
                .await?
        };
        if !verified {
            return Err(Error::bad_request("Invalid email or password"));
        }

        let model = user_to_view(user);
        let token = create_token(&*self.tokens, &model).await?;

        tracing::info!("[Auth] User logged in: {} ({})", model.name, model.id);
        Ok(AuthResponse { token, user: model })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<AuthMeRequest, UserView> for AuthMeHandler {
    async fn handle(&mut self, _req: AuthMeRequest) -> ActionResult<UserView> {
        let uid = operator_id().ok_or_else(|| Error::bad_request("Not authenticated"))?;

        let user = load_user_by_id(&mut self.ctx, &uid)
            .await?
            .ok_or_else(|| Error::bad_request("User not found"))?;

        Ok(user_to_view(user))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ForgotPasswordRequest, ForgotPasswordResponse> for ForgotPasswordHandler {
    async fn handle(&mut self, req: ForgotPasswordRequest) -> ActionResult<ForgotPasswordResponse> {
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
        self.ctx.add(token_record);

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

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ResetPasswordRequest, ResetPasswordResponse> for ResetPasswordHandler {
    async fn handle(&mut self, req: ResetPasswordRequest) -> ActionResult<ResetPasswordResponse> {
        if req.password.len() < 6 {
            return Err(Error::bad_request("Password must be at least 6 characters"));
        }

        let q = req.token.clone();
        let mut record = linq!(
            self.ctx.set::<PasswordResetToken>(),
            |t: PasswordResetToken| t.token == q
        )
        .first_or_default()
        .await
        .map_ef()?
        .ok_or_else(|| Error::bad_request("Invalid or expired reset token"))?;

        if record.used != 0 {
            return Err(Error::bad_request("Reset token already used"));
        }
        if now_secs() > record.expires_at {
            return Err(Error::bad_request("Reset token expired"));
        }

        let hashed = self.hasher.hash(&req.password).await?;

        let user_id = record.user_id.clone();
        let mut user = load_user_by_id(&mut self.ctx, &user_id)
            .await?
            .ok_or_else(|| Error::bad_request("User not found"))?;

        user.password_hash = hashed;
        user.updated_at = now_secs();
        record.used = 1;

        self.ctx.update(user);

        self.ctx.update(record);

        save_changes(&mut self.ctx).await?;

        tracing::info!("[Auth] Password reset completed for user {}", user_id);
        Ok(ResetPasswordResponse {
            message: "Password updated successfully. You can now sign in.".into(),
        })
    }
}
