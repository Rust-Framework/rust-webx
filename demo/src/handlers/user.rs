//! User handlers — auto-registered via `#[lrdi::inject_attr]` + `#[handler(inject)]`.

use std::sync::Arc;

use lref::{db_context::DbContext, prelude::*, provider::DbValue};
use lrwf::*;
use tokio::sync::Mutex;

use crate::contracts::user::*;
use crate::domain::user::{UserEntity, UserModel};

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<ListUsersRequest, Vec<UserModel>>)]
pub struct ListUsersHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<GetUserRequest, UserModel>)]
pub struct GetUserHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<CreateUserRequest, UserModel>)]
pub struct CreateUserHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<UpdateUserRequest, UserModel>)]
pub struct UpdateUserHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<DeleteUserRequest, String>)]
pub struct DeleteUserHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<InfoRequest, String>)]
pub struct InfoHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListUsersRequest, Vec<UserModel>> for ListUsersHandler {
    async fn handle(&self, _req: ListUsersRequest) -> Result<Vec<UserModel>> {
        let query = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<UserEntity>()
                .query()
                .order_by_desc_column("created_at")
        };
        let users = query
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(users.into_iter().map(UserModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetUserRequest, UserModel> for GetUserHandler {
    async fn handle(&self, req: GetUserRequest) -> Result<UserModel> {
        let query = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<UserEntity>()
                .query()
                .filter_column("id", "=", DbValue::String(req.id))
        };
        let user = query
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("User not found".into()))?;
        Ok(UserModel::from(user))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateUserRequest, UserModel> for CreateUserHandler {
    async fn handle(&self, req: CreateUserRequest) -> Result<UserModel> {
        let id = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_else(|_| "0".to_string());
        let sql = format!(
            "INSERT INTO users (id, name, email, password_hash, role, created_at) \
             VALUES ('{}', '{}', '{}', '', 'user', '{}')",
            id,
            req.name.replace('\'', "''"),
            req.email.replace('\'', "''"),
            now
        );
        {
            let ctx = self.ctx.lock().await;
            ctx.provider()
                .execute_migration_command(&sql)
                .await
                .map_err(|e| Error::Internal(format!("Failed to create user: {}", e)))?;
        }
        tracing::info!("[User] Created: {} ({})", req.name, id);
        Ok(UserModel {
            id,
            name: req.name,
            email: req.email,
            password_hash: String::new(),
            role: "user".to_string(),
            created_at: now,
        })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateUserRequest, UserModel> for UpdateUserHandler {
    async fn handle(&self, req: UpdateUserRequest) -> Result<UserModel> {
        let query = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<UserEntity>().query().filter_column(
                "id",
                "=",
                DbValue::String(req.id.clone()),
            )
        };
        let existing = query
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("User not found".into()))?;

        let new_name = req.name.unwrap_or(existing.name.clone());
        let new_email = req.email.unwrap_or(existing.email.clone());
        let sql = format!(
            "UPDATE users SET name='{}', email='{}' WHERE id='{}'",
            new_name.replace('\'', "''"),
            new_email.replace('\'', "''"),
            req.id
        );
        {
            let ctx = self.ctx.lock().await;
            ctx.provider()
                .execute_migration_command(&sql)
                .await
                .map_err(|e| Error::Internal(format!("Failed to update user: {}", e)))?;
        }
        tracing::info!("[User] Updated: {} ({})", new_name, req.id);
        Ok(UserModel {
            id: req.id,
            name: new_name,
            email: new_email,
            password_hash: existing.password_hash,
            role: existing.role,
            created_at: existing.created_at,
        })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteUserRequest, String> for DeleteUserHandler {
    async fn handle(&self, req: DeleteUserRequest) -> Result<String> {
        let sql = format!("DELETE FROM users WHERE id='{}'", req.id);
        {
            let ctx = self.ctx.lock().await;
            ctx.provider()
                .execute_migration_command(&sql)
                .await
                .map_err(|e| Error::Internal(format!("Failed to delete user: {}", e)))?;
        }
        tracing::info!("[User] Deleted: {}", req.id);
        Ok(format!("Deleted user {}", req.id))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<InfoRequest, String> for InfoHandler {
    async fn handle(&self, _req: InfoRequest) -> Result<String> {
        let query = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<UserEntity>().query()
        };
        let count = query
            .count()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(format!("Total users: {}", count))
    }
}
