//! User handlers — admin CRUD with audit fields and soft delete.

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*, provider::DbValue};
use rust_webapp::*;
use tokio::sync::Mutex;

use docbit_contracts::user::*;
use docbit_domain::entities::{RoleUser, User};

use crate::util::{now_secs, operator_id, parse_id};

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListUsersRequest, Vec<UserModel>>)]
pub struct ListUsersHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<GetUserRequest, UserModel>)]
pub struct GetUserHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<CreateUserRequest, UserModel>)]
pub struct CreateUserHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<UpdateUserRequest, UserModel>)]
pub struct UpdateUserHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<DeleteUserRequest, String>)]
pub struct DeleteUserHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<InfoRequest, String>)]
pub struct InfoHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[handler(inject)]
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

#[handler(inject)]
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

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateUserRequest, UserModel> for CreateUserHandler {
    async fn handle(&self, _: CreateUserRequest) -> Result<UserModel> {
        unreachable!("handle_with_claims is always called")
    }
    async fn handle_with_claims(
        &self,
        req: CreateUserRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<UserModel> {
        let op = operator_id(claims);
        let now = now_secs();
        let user = User {
            id: 0,
            name: req.name.clone(),
            email: req.email.clone(),
            password_hash: String::new(),
            created_id: op,
            created_at: now,
            updated_id: op,
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

        // 回查拿到自增 id，并分配默认 user 角色
        let created = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<User>()
                .query()
                .filter_column("email", "=", DbValue::String(req.email.clone()))
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

        tracing::info!("[User] Created: {} ({}) by {:?}", req.name, created.id, op);
        Ok(UserModel {
            id: created.id,
            name: created.name,
            email: created.email,
            roles: vec!["user".into()],
            created_at: created.created_at,
        })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateUserRequest, UserModel> for UpdateUserHandler {
    async fn handle(&self, _: UpdateUserRequest) -> Result<UserModel> {
        unreachable!("handle_with_claims is always called")
    }
    async fn handle_with_claims(
        &self,
        req: UpdateUserRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<UserModel> {
        let id = parse_id(&req.id)?;
        let mut user = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<User>()
                .query()
                .filter_column("id", "=", DbValue::I32(id))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("User not found".into()))?;

        if let Some(n) = req.name {
            user.name = n;
        }
        if let Some(e) = req.email {
            user.email = e;
        }
        user.updated_id = operator_id(claims);
        user.updated_at = now_secs();

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
        Ok(UserModel::from(updated))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteUserRequest, String> for DeleteUserHandler {
    async fn handle(&self, _: DeleteUserRequest) -> Result<String> {
        unreachable!("handle_with_claims is always called")
    }
    async fn handle_with_claims(
        &self,
        req: DeleteUserRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<String> {
        let id = parse_id(&req.id)?;
        let mut user = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<User>()
                .query()
                .filter_column("id", "=", DbValue::I32(id))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("User not found".into()))?;

        user.is_deleted = true;
        user.updated_id = operator_id(claims);
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

#[handler(inject)]
#[async_trait]
impl IRequestHandler<InfoRequest, String> for InfoHandler {
    async fn handle(&self, _: InfoRequest) -> Result<String> {
        let count = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<User>()
                .query()
                .filter_column("is_deleted", "=", DbValue::Bool(false))
                .count()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        Ok(format!("Total users: {}", count))
    }
}
