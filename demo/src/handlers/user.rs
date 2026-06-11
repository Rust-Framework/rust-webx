use lref::provider::DbValue;
use lref::query::QueryBuilder;
use lrwf::*;

use crate::contracts::user::*;
use crate::domain::user::{UserEntity, UserModel};

// ── IRequestHandler implementations — backed by lref ORM (SQLite) ──

#[derive(Default)]
pub struct ListUsersHandler;

#[derive(Default)]
pub struct GetUserHandler;

#[derive(Default)]
pub struct CreateUserHandler;

#[derive(Default)]
pub struct UpdateUserHandler;

#[derive(Default)]
pub struct DeleteUserHandler;

fn qb() -> QueryBuilder<UserEntity> {
    QueryBuilder::<UserEntity>::with_provider(
        "users",
        crate::handlers::startup::provider_dyn(),
    )
}

#[handler]
#[async_trait]
impl IRequestHandler<ListUsersRequest, Vec<UserModel>> for ListUsersHandler {
    async fn handle(&self, _: ListUsersRequest) -> Result<Vec<UserModel>> {
        let entities = qb()
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(entities.into_iter().map(UserModel::from).collect())
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<GetUserRequest, UserModel> for GetUserHandler {
    async fn handle(&self, req: GetUserRequest) -> Result<UserModel> {
        let entity = qb()
            .filter_column("id", "=", DbValue::String(req.id.clone()))
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound(format!("User not found: {}", req.id)))?;
        Ok(UserModel::from(entity))
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<CreateUserRequest, UserModel> for CreateUserHandler {
    async fn handle(&self, req: CreateUserRequest) -> Result<UserModel> {
        let id = uuid();
        let now = now_secs();
        let entity = UserEntity {
            id: id.clone(),
            name: req.name.clone(),
            email: req.email.clone(),
            password_hash: String::new(),
            role: "user".to_string(),
            created_at: now.clone(),
        };
        let sql = format!(
            "INSERT INTO users (id, name, email, password_hash, role, created_at) VALUES ('{}', '{}', '{}', '', 'user', '{}')",
            id, req.name.replace('\'', "''"), req.email.replace('\'', "''"), now
        );
        crate::handlers::startup::exec(&sql).await
            .map_err(|e| Error::Internal(format!("Failed to create user: {}", e)))?;
        let model = UserModel::from(entity);
        tracing::info!("[Event] User created: {} (id: {})", model.name, model.id);
        Ok(model)
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<UpdateUserRequest, UserModel> for UpdateUserHandler {
    async fn handle(&self, req: UpdateUserRequest) -> Result<UserModel> {
        let existing = qb()
            .filter_column("id", "=", DbValue::String(req.id.clone()))
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound(format!("User not found: {}", req.id)))?;

        let new_name = req.name.clone().unwrap_or(existing.name.clone());
        let new_email = req.email.clone().unwrap_or(existing.email.clone());

        let sql = format!(
            "UPDATE users SET name = '{}', email = '{}' WHERE id = '{}'",
            new_name.replace('\'', "''"),
            new_email.replace('\'', "''"),
            req.id
        );
        crate::handlers::startup::exec(&sql).await
            .map_err(|e| Error::Internal(format!("Failed to update user: {}", e)))?;

        let updated = UserEntity {
            id: existing.id,
            name: new_name,
            email: new_email,
            password_hash: existing.password_hash,
            role: existing.role,
            created_at: existing.created_at,
        };
        Ok(UserModel::from(updated))
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<DeleteUserRequest, String> for DeleteUserHandler {
    async fn handle(&self, req: DeleteUserRequest) -> Result<String> {
        let sql = format!("DELETE FROM users WHERE id = '{}'", req.id);
        crate::handlers::startup::exec(&sql).await
            .map_err(|e| Error::Internal(e.to_string()))?;
        tracing::info!("[Event] User deleted: {}", req.id);
        Ok(format!("User {} deleted", req.id))
    }
}

#[derive(Default)]
pub struct InfoHandler;

#[handler]
#[async_trait]
impl IRequestHandler<InfoRequest, String> for InfoHandler {
    async fn handle(&self, _: InfoRequest) -> Result<String> {
        let user_count = qb().count().await.unwrap_or(0);
        Ok(serde_json::json!({
            "name": "LRWF Demo API",
            "version": env!("CARGO_PKG_VERSION"),
            "users": user_count,
            "auth": {
                "register": "POST /api/auth/register",
                "login": "POST /api/auth/login",
                "me": "GET /api/auth/me"
            },
            "endpoints": [
                "GET /api/info",
                "GET /api/users (admin)", "GET /api/users/{id} (admin)",
                "POST /api/users (admin)", "PUT /api/users/{id} (admin)", "DELETE /api/users/{id} (admin)",
                "GET /api/products", "GET /api/products/{id}",
                "POST /api/products (admin)", "PUT /api/products/{id} (admin)", "DELETE /api/products/{id} (admin)",
                "GET /health",
                "GET /api/openapi.json", "GET /api/openapi.html"
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
