//! `POST /api/users` — create a user without a password (admin).

use serde::Deserialize;
use webx::*;

use super::models::UserModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct CreateUserRequest {
    pub name: String,
    pub email: String,
}

/// Create a new user with name and email address (admin-created, no password).
#[post("/api/users")]
#[authorize(role = "admin")]
impl IRequest<UserModel> for CreateUserRequest {}
