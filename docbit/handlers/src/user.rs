//! User handlers — admin CRUD with audit fields and soft delete.

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webapp::*;
use tokio::sync::Mutex;

use docbit_contracts::user::*;
use docbit_domain::entities::{RoleUser, User};
use docbit_domain::{ApplyTo, ToEntity, ToModel};

use crate::util::{now_secs, operator_id, parse_id};

#[derive(Inject)]
pub struct ListUsersHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct GetUserHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct CreateUserHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct UpdateUserHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct DeleteUserHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct InfoHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<ListUsersRequest, Vec<UserModel>> for ListUsersHandler {
    async fn handle(&self, _: ListUsersRequest) -> Result<Vec<UserModel>> {
        let users = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<User>(), |u: User| !u.is_deleted; include u.roles; order_by u.created_at desc)
                .to_list()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        Ok(users.into_iter().map(UserModel::from).collect())
    }
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<GetUserRequest, UserModel> for GetUserHandler {
    async fn handle(&self, req: GetUserRequest) -> Result<UserModel> {
        let id = parse_id(&req.id)?;
        let user = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<User>(), |u: User| u.id == id && !u.is_deleted; include u.roles)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("User not found".into()))?;
        Ok(UserModel::from(user))
    }
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<CreateUserRequest, UserModel> for CreateUserHandler {
    async fn handle(&self, req: CreateUserRequest) -> Result<UserModel> {
        let op = operator_id(req.claims.as_deref());
        let now = now_secs();
        let email = req.email.clone();
        let name = req.name.clone();
        let user = req.to_entity(op.unwrap_or(0), now);
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<User>().add(user);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to create user: {}", e)))?;
        }

        // 回查拿到自增 id，并分配默认 user 角色
        let created = {
            let mut ctx = self.ctx.lock().await;
            let q = email.clone();
            linq!(ctx.set::<User>(), |u: User| u.email == q)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::Internal("User disappeared after insert".into()))?;

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

        tracing::info!("[User] Created: {} ({}) by {:?}", name, created.id, op);
        Ok(UserModel {
            id: created.id,
            name: created.name,
            email: created.email,
            roles: vec!["user".into()],
            created_at: created.created_at,
        })
    }
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<UpdateUserRequest, UserModel> for UpdateUserHandler {
    async fn handle(&self, req: UpdateUserRequest) -> Result<UserModel> {
        let id = parse_id(&req.id)?;
        let mut user = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<User>()
                .query()
                .find(id)
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("User not found".into()))?;

        let op = operator_id(req.claims.as_deref()).unwrap_or(0);
        req.apply_to(&mut user, op, now_secs());

        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<User>().update(user);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to update user: {}", e)))?;
        }

        // 回查含角色
        let updated = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<User>(), |u: User| u.id == id && !u.is_deleted; include u.roles)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("User not found after update".into()))?;
        Ok(updated.to_model())
    }
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<DeleteUserRequest, String> for DeleteUserHandler {
    async fn handle(&self, req: DeleteUserRequest) -> Result<String> {
        let id = parse_id(&req.id)?;
        let mut user = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<User>()
                .query()
                .find(id)
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("User not found".into()))?;

        user.is_deleted = true;
        user.updated_id = operator_id(req.claims.as_deref());
        user.updated_at = now_secs();
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<User>().update(user);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }
        tracing::info!("[User] Soft-deleted: {}", id);
        Ok(format!("Deleted user {}", id))
    }
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<InfoRequest, String> for InfoHandler {
    async fn handle(&self, _: InfoRequest) -> Result<String> {
        let count = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<User>(), |u: User| !u.is_deleted; count)
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        Ok(format!("Total users: {}", count))
    }
}
