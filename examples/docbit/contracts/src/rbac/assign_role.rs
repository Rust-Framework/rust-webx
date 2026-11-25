//! `POST /api/role-users` — assign a role to a user (admin, idempotent).

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct AssignRoleRequest {
    pub user_id: String,
    pub role_id: String,
}

#[post("/api/role-users")]
#[authorize(role = "admin")]
impl IRequest<String> for AssignRoleRequest {}
