//! `GET /api/users/{id}` — one user (admin).

use serde::Deserialize;
use webx::*;

use super::models::UserModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetUserRequest {
    pub id: String,
}

/// Get a single user by their unique ID.
#[get("/api/users/{id}")]
#[authorize(role = "admin")]
impl IRequest<UserModel> for GetUserRequest {}
