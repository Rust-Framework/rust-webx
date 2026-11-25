//! `DELETE /api/role-users/{user_id}/{role_id}` — revoke a role (admin).

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct RevokeRoleRequest {
    pub user_id: String,
    pub role_id: String,
}

#[delete("/api/role-users/{user_id}/{role_id}")]
#[authorize(role = "admin")]
impl IRequest<String> for RevokeRoleRequest {}
