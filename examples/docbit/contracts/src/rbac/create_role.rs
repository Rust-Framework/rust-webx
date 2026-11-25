//! `POST /api/roles` — create a role (admin).

use serde::Deserialize;
use webx::*;

use super::models::RoleModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct CreateRoleRequest {
    pub name: String,
    pub description: String,
}

#[post("/api/roles")]
#[authorize(role = "admin")]
impl IRequest<RoleModel> for CreateRoleRequest {}
