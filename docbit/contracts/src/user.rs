//! User contracts — admin CRUD endpoints.
//!
//! Migrated from docbit/src/contracts/user.rs with UserModel changes:
//! - `id` changed from String to i32
//! - `password_hash` field removed (never exposed to clients)
//! - `role: String` removed (replaced by multi-role RBAC)
//! - `roles: Vec<String>` added
//! - `created_at` changed from String to i64

use rust_webapp::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserModel {
    pub id: i32,
    pub name: String,
    pub email: String,
    pub roles: Vec<String>,
    pub created_at: i64,
}

#[derive(Default)]
pub struct InfoRequest;

/// Get API server information including version and endpoint count
#[get("/api/info")]
impl IRequest<String> for InfoRequest {}

#[derive(Default)]
pub struct ListUsersRequest;

/// Returns a list of all registered users
#[get("/api/users")]
#[authorize(role = "admin")]
impl IRequest<Vec<UserModel>> for ListUsersRequest {}

#[derive(Default)]
pub struct GetUserRequest {
    pub id: String,
}

/// Get a single user by their unique ID
#[get("/api/users/{id}")]
#[authorize(role = "admin")]
impl IRequest<UserModel> for GetUserRequest {}

#[derive(Default, Deserialize)]
pub struct CreateUserRequest {
    #[serde(skip)]
    pub claims: Option<Box<dyn IClaims>>,
    pub name: String,
    pub email: String,
}
impl_claims_carrier!(CreateUserRequest);

/// Create a new user with name and email address (admin-created, no password)
#[post("/api/users")]
#[authorize(role = "admin")]
impl IRequest<UserModel> for CreateUserRequest {}

#[derive(Default, Deserialize)]
pub struct UpdateUserRequest {
    #[serde(skip)]
    pub claims: Option<Box<dyn IClaims>>,
    pub id: String,
    pub name: Option<String>,
    pub email: Option<String>,
}
impl_claims_carrier!(UpdateUserRequest);

/// Update an existing user's name and/or email
#[put("/api/users/{id}")]
#[authorize(role = "admin")]
impl IRequest<UserModel> for UpdateUserRequest {}

#[derive(Default)]
pub struct DeleteUserRequest {
    pub claims: Option<Box<dyn IClaims>>,
    pub id: String,
}
impl_claims_carrier!(DeleteUserRequest);

/// Delete a user by their unique ID
#[delete("/api/users/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteUserRequest {}
