//! User handlers — auto-registered via `#[lrdi::inject_attr]` + `#[handler(inject)]`.

use std::sync::Arc;

use lref::provider::DbValue;
use lrwf::*;

use crate::contracts::user::*;
use crate::domain::db_context::AppDbContext;
use crate::domain::user::{UserEntity, UserModel};

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<ListUsersRequest, Vec<UserModel>>)]
pub struct ListUsersHandler {
    ctx: Arc<AppDbContext>,
}

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<GetUserRequest, UserModel>)]
pub struct GetUserHandler {
    ctx: Arc<AppDbContext>,
}

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<CreateUserRequest, UserModel>)]
pub struct CreateUserHandler {
    ctx: Arc<AppDbContext>,
}

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<UpdateUserRequest, UserModel>)]
pub struct UpdateUserHandler {
    ctx: Arc<AppDbContext>,
}

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<DeleteUserRequest, String>)]
pub struct DeleteUserHandler {
    ctx: Arc<AppDbContext>,
}

#[lrdi::inject_attr(singleton, as = dyn IRequestHandler<InfoRequest, String>)]
pub struct InfoHandler {
    ctx: Arc<AppDbContext>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListUsersRequest, Vec<UserModel>> for ListUsersHandler {
    async fn handle(&self, _: ListUsersRequest) -> Result<Vec<UserModel>> {
        let entities = self
            .ctx
            .set::<UserEntity>()
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(entities.into_iter().map(UserModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetUserRequest, UserModel> for GetUserHandler {
    async fn handle(&self, req: GetUserRequest) -> Result<UserModel> {
        let entity = self
            .ctx
            .set::<UserEntity>()
            .filter_column("id", "=", DbValue::String(req.id.clone()))
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound(format!("User not found: {}", req.id)))?;
        Ok(UserModel::from(entity))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateUserRequest, UserModel> for CreateUserHandler {
    async fn handle(&self, req: CreateUserRequest) -> Result<UserModel> {
        let id = uuid();
        let now = now_secs();
        let sql = format!(
            "INSERT INTO users (id, name, email, password_hash, role, created_at) \
             VALUES ('{}', '{}', '{}', '', 'user', '{}')",
            id,
            req.name.replace('\'', "''"),
            req.email.replace('\'', "''"),
            now
        );
        self.ctx
            .execute(&sql)
            .await
            .map_err(|e| Error::Internal(format!("Failed to create user: {}", e)))?;
        let model = UserModel {
            id,
            name: req.name,
            email: req.email,
            password_hash: String::new(),
            role: "user".to_string(),
            created_at: now,
        };
        tracing::info!("[Event] User created: {} (id: {})", model.name, model.id);
        Ok(model)
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateUserRequest, UserModel> for UpdateUserHandler {
    async fn handle(&self, req: UpdateUserRequest) -> Result<UserModel> {
        let existing = self
            .ctx
            .set::<UserEntity>()
            .filter_column("id", "=", DbValue::String(req.id.clone()))
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound(format!("User not found: {}", req.id)))?;
        let new_name = req.name.unwrap_or(existing.name.clone());
        let new_email = req.email.unwrap_or(existing.email.clone());
        let sql = format!(
            "UPDATE users SET name = '{}', email = '{}' WHERE id = '{}'",
            new_name.replace('\'', "''"),
            new_email.replace('\'', "''"),
            req.id
        );
        self.ctx
            .execute(&sql)
            .await
            .map_err(|e| Error::Internal(format!("Failed to update user: {}", e)))?;
        Ok(UserModel::from(UserEntity {
            id: existing.id,
            name: new_name,
            email: new_email,
            password_hash: existing.password_hash,
            role: existing.role,
            created_at: existing.created_at,
        }))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteUserRequest, String> for DeleteUserHandler {
    async fn handle(&self, req: DeleteUserRequest) -> Result<String> {
        let sql = format!("DELETE FROM users WHERE id = '{}'", req.id);
        self.ctx
            .execute(&sql)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        tracing::info!("[Event] User deleted: {}", req.id);
        Ok(format!("User {} deleted", req.id))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<InfoRequest, String> for InfoHandler {
    async fn handle(&self, _: InfoRequest) -> Result<String> {
        let user_count = self.ctx.set::<UserEntity>().count().await.unwrap_or(0);
        Ok(serde_json::json!({
            "name": "LRWF Demo API", "version": env!("CARGO_PKG_VERSION"), "users": user_count,
            "auth": { "register": "POST /api/auth/register", "login": "POST /api/auth/login", "me": "GET /api/auth/me" },
            "endpoints": [
                "GET /api/info", "GET /api/users (admin)", "GET /api/users/{id} (admin)",
                "POST /api/users (admin)", "PUT /api/users/{id} (admin)", "DELETE /api/users/{id} (admin)",
                "GET /api/products", "GET /api/products/{id}",
                "POST /api/products (admin)", "PUT /api/products/{id} (admin)", "DELETE /api/products/{id} (admin)",
                "GET /health", "GET /api/openapi.json", "GET /api/openapi.html"
            ]
        }).to_string())
    }
}

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| format!("{:x}", d.as_nanos()))
        .unwrap_or_else(|_| "0".to_string())
}
fn now_secs() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}
