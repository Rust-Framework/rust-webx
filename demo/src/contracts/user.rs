use lrwf::*;
use crate::domain::user::UserModel;

// ── IRequest definitions — the API contract ──

pub struct ListUsersRequest;

#[get("/api/users")]
impl IRequest<Vec<UserModel>> for ListUsersRequest {}

pub struct GetUserRequest {
    pub id: String,
}

#[get("/api/users/{id}")]
impl IRequest<UserModel> for GetUserRequest {}

#[derive(serde::Deserialize)]
pub struct CreateUserRequest {
    pub name: String,
    pub email: String,
}

#[post("/api/users")]
impl IRequest<UserModel> for CreateUserRequest {}

#[derive(serde::Deserialize)]
pub struct UpdateUserRequest {
    pub id: String,
    pub name: Option<String>,
    pub email: Option<String>,
}

#[put("/api/users/{id}")]
impl IRequest<UserModel> for UpdateUserRequest {}

pub struct DeleteUserRequest {
    pub id: String,
}

#[delete("/api/users/{id}")]
impl IRequest<String> for DeleteUserRequest {}

// ── IEventRequest definitions — events are contracts too ──

#[derive(Clone)]
pub struct UserCreatedEvent {
    pub user_id: String,
    pub user_name: String,
}

impl IEventRequest for UserCreatedEvent {}

#[derive(Clone)]
pub struct UserDeletedEvent {
    pub user_id: String,
}

impl IEventRequest for UserDeletedEvent {}
