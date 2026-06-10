use lrwf::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::contracts::user::*;
use crate::domain::user::UserModel;

// =========================================================================
// In-memory UserRepository
// =========================================================================

struct UserRepository {
    users: Mutex<HashMap<String, UserModel>>,
}

impl UserRepository {
    fn new() -> Self {
        let repo = Self {
            users: Mutex::new(HashMap::new()),
        };
        repo.create("Alice", "alice@example.com");
        repo.create("Bob", "bob@example.com");
        repo.create("Charlie", "charlie@example.com");
        repo
    }

    fn list(&self) -> Vec<UserModel> {
        self.users
            .lock()
            .map(|g| g.values().cloned().collect())
            .unwrap_or_default()
    }

    fn get(&self, id: &str) -> Option<UserModel> {
        self.users.lock().ok()?.get(id).cloned()
    }

    fn create(&self, name: &str, email: &str) -> UserModel {
        let id = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let user = UserModel {
            id: id.clone(),
            name: name.to_string(),
            email: email.to_string(),
            password_hash: String::new(),
            role: "user".to_string(),
            created_at: now_string(),
        };
        self.users
            .lock()
            .map(|mut g| {
                g.insert(id, user.clone());
            })
            .ok();
        user
    }

    fn create_with_password(
        &self,
        name: &str,
        email: &str,
        password_hash: &str,
        role: &str,
    ) -> UserModel {
        let id = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let user = UserModel {
            id: id.clone(),
            name: name.to_string(),
            email: email.to_string(),
            password_hash: password_hash.to_string(),
            role: role.to_string(),
            created_at: now_string(),
        };
        self.users
            .lock()
            .map(|mut g| {
                g.insert(id, user.clone());
            })
            .ok();
        user
    }

    fn find_by_email(&self, email: &str) -> Option<UserModel> {
        let users = self.users.lock().ok()?;
        users.values().find(|u| u.email == email).cloned()
    }

    fn update(&self, id: &str, name: Option<&str>, email: Option<&str>) -> Option<UserModel> {
        let mut users = self.users.lock().ok()?;
        if let Some(user) = users.get_mut(id) {
            if let Some(n) = name {
                user.name = n.to_string();
            }
            if let Some(e) = email {
                user.email = e.to_string();
            }
            Some(user.clone())
        } else {
            None
        }
    }

    fn delete(&self, id: &str) -> bool {
        self.users
            .lock()
            .map(|mut g| g.remove(id).is_some())
            .unwrap_or(false)
    }
}

fn now_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}", ts)
}

static REPO: OnceLock<Arc<UserRepository>> = OnceLock::new();

#[allow(dead_code)]
fn init_repo() {
    let _ = REPO.set(Arc::new(UserRepository::new()));
}

pub fn repo() -> &'static Arc<UserRepository> {
    REPO.get().unwrap_or_else(|| {
        tracing::warn!(
            "[LRWF Demo] UserRepository accessed before initialization; auto-initializing."
        );
        let _ = REPO.set(Arc::new(UserRepository::new()));
        REPO.get().expect("UserRepository initialization failed")
    })
}

// =========================================================================
// IRequestHandler implementations
// =========================================================================

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

#[handler]
#[async_trait]
impl IRequestHandler<ListUsersRequest, Vec<UserModel>> for ListUsersHandler {
    async fn handle(&self, _: ListUsersRequest) -> Result<Vec<UserModel>> {
        Ok(repo().list())
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<GetUserRequest, UserModel> for GetUserHandler {
    async fn handle(&self, req: GetUserRequest) -> Result<UserModel> {
        repo()
            .get(&req.id)
            .ok_or_else(|| Error::NotFound(format!("User not found: {}", req.id)))
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<CreateUserRequest, UserModel> for CreateUserHandler {
    async fn handle(&self, req: CreateUserRequest) -> Result<UserModel> {
        let user = repo().create(&req.name, &req.email);
        tracing::info!("[Event] User created: {} (id: {})", user.name, user.id);
        Ok(user)
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<UpdateUserRequest, UserModel> for UpdateUserHandler {
    async fn handle(&self, req: UpdateUserRequest) -> Result<UserModel> {
        repo()
            .update(&req.id, req.name.as_deref(), req.email.as_deref())
            .ok_or_else(|| Error::NotFound(format!("User not found: {}", req.id)))
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<DeleteUserRequest, String> for DeleteUserHandler {
    async fn handle(&self, req: DeleteUserRequest) -> Result<String> {
        if repo().delete(&req.id) {
            tracing::info!("[Event] User deleted: {}", req.id);
            Ok(format!("User {} deleted", req.id))
        } else {
            Err(Error::NotFound(format!("User not found: {}", req.id)))
        }
    }
}

#[derive(Default)]
pub struct InfoHandler;

#[handler]
#[async_trait]
impl IRequestHandler<InfoRequest, String> for InfoHandler {
    async fn handle(&self, _: InfoRequest) -> Result<String> {
        Ok(serde_json::json!({
            "name": "LRWF Demo API",
            "version": env!("CARGO_PKG_VERSION"),
            "users": repo().list().len(),
            "endpoints": [
                "GET /api/info",
                "GET /api/users", "GET /api/users/{id}", "POST /api/users", "PUT /api/users/{id}", "DELETE /api/users/{id}",
                "GET /api/products", "GET /api/products/{id}", "POST /api/products", "PUT /api/products/{id}", "DELETE /api/products/{id}",
                "GET /health",
                "GET /api/openapi.json", "GET /api/openapi.html"
            ]
        }).to_string())
    }
}

// =========================================================================
// IEventHandler implementations
// =========================================================================

/// Logs user lifecycle events to stdout.
#[derive(Default)]
#[allow(dead_code)]
pub struct UserEventLogger;

#[async_trait]
impl IEventHandler<UserCreatedEvent> for UserEventLogger {
    async fn handle(&self, event: UserCreatedEvent) -> Result<()> {
        tracing::info!(
            "* [Event] User created: {} ({})",
            event.user_name,
            event.user_id
        );
        Ok(())
    }
}

#[async_trait]
impl IEventHandler<UserDeletedEvent> for UserEventLogger {
    async fn handle(&self, event: UserDeletedEvent) -> Result<()> {
        tracing::info!("- [Event] User deleted: {}", event.user_id);
        Ok(())
    }
}

/// Call once at startup to initialize the repository.
#[allow(dead_code)]
pub fn init() {
    init_repo();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_create_and_get() {
        let repo = UserRepository::new();
        let user = repo.create("Alice", "alice@example.com");
        assert_eq!(user.name, "Alice");
        assert_eq!(user.email, "alice@example.com");
        assert!(!user.id.is_empty());

        let fetched = repo.get(&user.id);
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().name, "Alice");
    }

    #[test]
    fn repo_get_nonexistent_returns_none() {
        let repo = UserRepository::new();
        assert!(repo.get("nonexistent").is_none());
    }

    #[test]
    fn repo_list_returns_all_users() {
        let repo = UserRepository::new();
        assert_eq!(repo.list().len(), 3);
        repo.create("User1", "u1@test.com");
        repo.create("User2", "u2@test.com");
        assert_eq!(repo.list().len(), 5);
    }

    #[test]
    fn repo_update_existing_user() {
        let repo = UserRepository::new();
        let user = repo.create("Original", "o@test.com");
        let updated = repo.update(&user.id, Some("Updated"), None);
        assert!(updated.is_some());
        assert_eq!(updated.unwrap().name, "Updated");

        let fetched = repo.get(&user.id).unwrap();
        assert_eq!(fetched.name, "Updated");
        assert_eq!(fetched.email, "o@test.com");
    }

    #[test]
    fn repo_update_nonexistent_returns_none() {
        let repo = UserRepository::new();
        assert!(repo.update("nonexistent", Some("X"), None).is_none());
    }

    #[test]
    fn repo_delete_existing_user() {
        let repo = UserRepository::new();
        let user = repo.create("ToDelete", "d@test.com");
        assert!(repo.delete(&user.id));
        assert!(repo.get(&user.id).is_none());
    }

    #[test]
    fn repo_delete_nonexistent_returns_false() {
        let repo = UserRepository::new();
        assert!(!repo.delete("nonexistent"));
    }
}
