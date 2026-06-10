use crate::domain::user::UserModel;
use lrwf::*;

// ── IRequest definitions — the API contract ──

pub struct InfoRequest;

/// Get API server information including version and endpoint count
#[get("/api/info")]
impl IRequest<String> for InfoRequest {}

pub struct ListUsersRequest;

/// Returns a list of all registered users
#[get("/api/users")]
#[authorize(role = "admin")]
impl IRequest<Vec<UserModel>> for ListUsersRequest {}

pub struct GetUserRequest {
    pub id: String,
}

/// Get a single user by their unique ID
#[get("/api/users/{id}")]
#[authorize(role = "admin")]
impl IRequest<UserModel> for GetUserRequest {}

#[derive(serde::Deserialize)]
pub struct CreateUserRequest {
    pub name: String,
    pub email: String,
}

/// Create a new user with name and email address
#[post("/api/users")]
#[authorize(role = "admin")]
impl IRequest<UserModel> for CreateUserRequest {}

#[derive(serde::Deserialize)]
pub struct UpdateUserRequest {
    pub id: String,
    pub name: Option<String>,
    pub email: Option<String>,
}

/// Update an existing user's name and/or email
#[put("/api/users/{id}")]
#[authorize(role = "admin")]
impl IRequest<UserModel> for UpdateUserRequest {}

pub struct DeleteUserRequest {
    pub id: String,
}

/// Delete a user by their unique ID
#[delete("/api/users/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteUserRequest {}

// ── IEventRequest definitions — events are contracts too ──

#[derive(Clone)]
#[allow(dead_code)]
pub struct UserCreatedEvent {
    pub user_id: String,
    pub user_name: String,
}

impl IEventRequest for UserCreatedEvent {}

#[derive(Clone)]
#[allow(dead_code)]
pub struct UserDeletedEvent {
    pub user_id: String,
}

impl IEventRequest for UserDeletedEvent {}
