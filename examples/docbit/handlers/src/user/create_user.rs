//! `POST /api/users` — create an account without a password (admin).
//!
//! The account gets the `user` role; it can only sign in after a password reset.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::user::{CreateUserRequest, UserModel};
use docbit_domain::entities::{RoleUser, User};
use docbit_domain::{new_id, seed_ids};

use crate::common::operator_id;

#[derive(Inject)]
pub struct CreateUserHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateUserRequest, UserModel> for CreateUserHandler {
    async fn handle(&mut self, req: CreateUserRequest) -> ActionResult<UserModel> {
        // ── 变更：用户行 + 默认 `user` 角色链接，同一事务、两批 flush ──
        //
        // `role_users` 只用 `#[foreign_key]` 关联 `users`/`roles`（没有导航字段），
        // 单批 save 的插入顺序按元数据顺序，会先写链接行而违反外键；先插用户并
        // flush，再插链接，两次 flush 仍在同一事务、只提交一次。
        let user = User::try_from(req)?;
        let user_id = user.id.clone();
        let role_link = RoleUser {
            id: new_id(),
            user_id: user_id.clone(),
            role_id: seed_ids::ROLE_USER.to_string(),
            created_at: user.created_at,
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

        // ── 响应 ──
        let saved = crate::ef_require_by_id!(
            self.ctx,
            User,
            user_id,
            Error::not_found("User not found after create");
            include row.roles
        );

        tracing::info!(
            "[User] Created: {} ({}) by {:?}",
            saved.name,
            saved.id,
            operator_id()
        );
        Ok(UserModel::from(&saved))
    }
}
