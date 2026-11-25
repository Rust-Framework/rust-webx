//! `DELETE /api/users/{id}` — soft delete (admin).

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct DeleteUserRequest {
    pub id: String,
}

/// Delete a user by their unique ID.
#[delete("/api/users/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteUserRequest {}
