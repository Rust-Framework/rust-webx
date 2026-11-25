//! `POST /api/auth/register` — create the account, grant `user`, mint a JWT.

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::auth::{AuthResponse, RegisterRequest, UserModel};
use docbit_domain::entities::{RoleUser, User};
use docbit_domain::{new_id, seed_ids};

use crate::common::now_secs;

use super::queries::{create_token, load_user_by_email};

#[derive(Inject)]
pub struct RegisterHandler {
    #[inject(owned)]
    ctx: DbContext,
    #[inject]
    tokens: Arc<dyn ITokenIssuer>,
    #[inject]
    hasher: Arc<dyn IPasswordHasher>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<RegisterRequest, AuthResponse> for RegisterHandler {
    async fn handle(&mut self, req: RegisterRequest) -> ActionResult<AuthResponse> {
        // ── 校验：邮箱未被占用 ──
        if load_user_by_email(&mut self.ctx, &req.email)
            .await?
            .is_some()
        {
            return Err(Error::validation("Email already registered"));
        }

        // ── 变更：用户行 + 默认 `user` 角色链接，同一事务、两批 flush ──
        //
        // `role_users` 只用 `#[foreign_key]` 关联 `users`/`roles`（没有导航字段），
        // 单批 save 的插入顺序按元数据顺序，会先写链接行再写用户行而违反外键；
        // 因此先插用户并 flush，再插链接，两次 flush 仍在同一事务、只提交一次。
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
        let role_link = RoleUser {
            id: new_id(),
            user_id: user_id.clone(),
            role_id: seed_ids::ROLE_USER.to_string(),
            created_at: now,
        };

        self.ctx
            .use_transaction_generic(move |ctx| {
                Box::pin(async move {
                    ctx.add(user);
                    ctx.save_changes().await?;

                    ctx.add(role_link);
                    ctx.save_changes().await?;
                    Ok::<(), Error>(())
                })
            })
            .await?;

        // ── 响应：重新加载角色导航后签发 token ──
        let saved = crate::ef_require_by_id!(
            self.ctx,
            User,
            user_id,
            Error::not_found("User not found after register");
            include row.roles
        );
        let model = UserModel::from(&saved);
        let token = create_token(&*self.tokens, &model).await?;

        tracing::info!("[Auth] User registered: {} ({})", model.name, model.id);
        Ok(AuthResponse { token, user: model })
    }
}
