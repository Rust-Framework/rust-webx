use lrwf::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::domain::user::UserModel;
use crate::contracts::user::*;

// =========================================================================
// In-memory UserRepository
// =========================================================================

struct UserRepository {
    users: Mutex<HashMap<String, UserModel>>,
}

impl UserRepository {
    fn new() -> Self {
        Self {
            users: Mutex::new(HashMap::new()),
        }
    }

    fn list(&self) -> Vec<UserModel> {
        self.users.lock().unwrap().values().cloned().collect()
    }

    fn get(&self, id: &str) -> Option<UserModel> {
        self.users.lock().unwrap().get(id).cloned()
    }

    fn create(&self, name: &str, email: &str) -> UserModel {
        let id = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let user = UserModel {
            id: id.clone(),
            name: name.to_string(),
            email: email.to_string(),
            role: "user".to_string(),
            created_at: now_string(),
        };
        self.users.lock().unwrap().insert(id, user.clone());
        user
    }

    fn update(
        &self,
        id: &str,
        name: Option<&str>,
        email: Option<&str>,
    ) -> Option<UserModel> {
        let mut users = self.users.lock().unwrap();
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
        self.users.lock().unwrap().remove(id).is_some()
    }
}

fn now_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    format!("{}", ts)
}

static REPO: OnceLock<Arc<UserRepository>> = OnceLock::new();

fn init_repo() {
    let _ = REPO.set(Arc::new(UserRepository::new()));
}

fn repo() -> &'static Arc<UserRepository> {
    REPO.get().expect("UserRepository not initialized")
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
        println!(
            "[Event] User created: {} (id: {})",
            user.name, user.id
        );
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
            println!("[Event] User deleted: {}", req.id);
            Ok(format!("User {} deleted", req.id))
        } else {
            Err(Error::NotFound(format!("User not found: {}", req.id)))
        }
    }
}

// =========================================================================
// IEventHandler implementations
// =========================================================================

/// Logs user lifecycle events to stdout.
#[derive(Default)]
pub struct UserEventLogger;

#[async_trait]
impl IEventHandler<UserCreatedEvent> for UserEventLogger {
    async fn handle(&self, event: UserCreatedEvent) -> Result<()> {
        println!("* [Event] User created: {} ({})", event.user_name, event.user_id);
        Ok(())
    }
}

#[async_trait]
impl IEventHandler<UserDeletedEvent> for UserEventLogger {
    async fn handle(&self, event: UserDeletedEvent) -> Result<()> {
        println!("- [Event] User deleted: {}", event.user_id);
        Ok(())
    }
}

/// Call once at startup to initialize the repository.
pub fn init() {
    init_repo();
}
