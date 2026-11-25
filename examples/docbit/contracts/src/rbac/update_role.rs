//! `PUT /api/roles/{id}` — update a role (admin).

use serde::Deserialize;
use webx::*;

use super::models::RoleModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct UpdateRoleRequest {
    #[frompath]
    #[serde(default)]
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
}

#[put("/api/roles/{id}")]
#[authorize(role = "admin")]
impl IRequest<RoleModel> for UpdateRoleRequest {}
