//! `DELETE /api/roles/{id}` — soft delete a role (admin).

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct DeleteRoleRequest {
    pub id: String,
}

#[delete("/api/roles/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteRoleRequest {}
