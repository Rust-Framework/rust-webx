//! `PUT /api/users/{id}` — update name/email (admin).

use serde::Deserialize;
use webx::*;

use super::models::UserModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct UpdateUserRequest {
    #[frompath]
    #[serde(default)]
    pub id: String,
    pub name: Option<String>,
    pub email: Option<String>,
}

/// Update an existing user's name and/or email.
#[put("/api/users/{id}")]
#[authorize(role = "admin")]
impl IRequest<UserModel> for UpdateUserRequest {}
