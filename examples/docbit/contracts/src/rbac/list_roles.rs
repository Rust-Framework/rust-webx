//! `GET /api/roles` — every role (admin).

use serde::Deserialize;
use webx::*;

use super::models::RoleModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListRolesRequest;

#[get("/api/roles")]
#[authorize(role = "admin")]
impl IRequest<Vec<RoleModel>> for ListRolesRequest {}
