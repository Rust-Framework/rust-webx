//! User handlers — admin CRUD with audit fields and soft delete.
//!
//! 每个 handler 持有 owned `DbContext`，`handle(&mut self, ...)` 直接操作 `self.ctx`。

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use docbit_contracts::user::*;
use docbit_domain::entities::{RoleUser, User};
use docbit_domain::{new_id, seed_ids, ApplyTo, ToEntity, ToModel};

use crate::db::{save_changes, EfResultExt};
use crate::util::{now_secs, operator_id, parse_id};

#[derive(Inject)]
pub struct ListUsersHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct GetUserHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct CreateUserHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct UpdateUserHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct DeleteUserHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct InfoHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListUsersRequest, Vec<UserModel>> for ListUsersHandler {
    async fn handle(&mut self, _: ListUsersRequest) -> Result<Vec<UserModel>> {
        let users = linq!(self.ctx.set::<User>(); include u.roles; order_by u.created_at desc)
            .to_list()
            .await
            .map_ef()?;

        Ok(users.into_iter().map(UserModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetUserRequest, UserModel> for GetUserHandler {
    async fn handle(&mut self, req: GetUserRequest) -> Result<UserModel> {
        let id = parse_id(&req.id)?;
        let q = id.clone();

        let user = linq!(self.ctx.set::<User>(), |u: User| u.id == q; include u.roles)
            .first_or_default()
            .await
            .map_ef()?
            .ok_or_else(|| Error::NotFound("User not found".into()))?;

        Ok(UserModel::from(user))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateUserRequest, UserModel> for CreateUserHandler {
    async fn handle(&mut self, req: CreateUserRequest) -> Result<UserModel> {
        let op = operator_id(req.claims.as_deref());
        let now = now_secs();
        let user_id = new_id();
        let name = req.name.clone();
        let email = req.email.clone();

        let user = req.to_entity(user_id.clone(), op.clone(), now);
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

        save_changes(&mut self.ctx).await?;

        tracing::info!("[User] Created: {} ({}) by {:?}", name, user_id, op);
        Ok(UserModel {
            id: user_id,
            name,
            email,
            roles: vec!["user".into()],
            created_at: now,
        })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateUserRequest, UserModel> for UpdateUserHandler {
    async fn handle(&mut self, req: UpdateUserRequest) -> Result<UserModel> {
        let id = parse_id(&req.id)?;

        let mut user = self
            .ctx
            .set::<User>()
            .query()
            .find(id.clone())
            .await
            .map_ef()?
            .ok_or_else(|| Error::NotFound("User not found".into()))?;

        let op = operator_id(req.claims.as_deref());
        req.apply_to(&mut user, op, now_secs());

        let set = self.ctx.set::<User>();
        set.update(user);

        save_changes(&mut self.ctx).await?;

        let q = id.clone();
        let updated = linq!(self.ctx.set::<User>(), |u: User| u.id == q; include u.roles)
            .first_or_default()
            .await
            .map_ef()?
            .ok_or_else(|| Error::NotFound("User not found after update".into()))?;

        Ok(updated.to_model())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteUserRequest, String> for DeleteUserHandler {
    async fn handle(&mut self, req: DeleteUserRequest) -> Result<String> {
        let id = parse_id(&req.id)?;

        let mut user = self
            .ctx
            .set::<User>()
            .query()
            .find(id.clone())
            .await
            .map_ef()?
            .ok_or_else(|| Error::NotFound("User not found".into()))?;

        user.is_deleted = true;
        user.updated_id = operator_id(req.claims.as_deref());
        user.updated_at = now_secs();

        let set = self.ctx.set::<User>();
        set.update(user);

        save_changes(&mut self.ctx).await?;

        tracing::info!("[User] Soft-deleted: {}", id);
        Ok(format!("Deleted user {}", id))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<InfoRequest, String> for InfoHandler {
    async fn handle(&mut self, _: InfoRequest) -> Result<String> {
        let count = linq!(self.ctx.set::<User>(); count)
            .await
            .map_ef()?;

        Ok(format!("Total users: {}", count))
    }
}
