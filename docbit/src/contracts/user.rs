use rust_webapp::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserModel {
    pub id: String,
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub password_hash: String,
    pub role: String,
    pub created_at: String,
}

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

#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub name: String,
    pub email: String,
}

/// Create a new user with name and email address
#[post("/api/users")]
#[authorize(role = "admin")]
impl IRequest<UserModel> for CreateUserRequest {}

#[derive(Deserialize)]
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
