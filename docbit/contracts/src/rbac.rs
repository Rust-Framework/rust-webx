//! RBAC contracts — roles, resources (通用资源模型), and authorizations.

use rust_webx::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleModel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 资源模型：通用 `type + value + properties` 三段式。
/// - `type`：应用/模块/页面/操作/数据/其他
/// - `value`：页面/操作类型时为路由
/// - `properties`：配置属性 JSON，如 `{"method":"GET"}`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceModel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub r#type: String,
    pub value: String,
    pub properties: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizeModel {
    pub id: String,
    pub role_id: String,
    pub resource_id: String,
}

// ── Role CRUD ──

#[derive(Default)]
pub struct ListRolesRequest;

#[get("/api/roles")]
#[authorize(role = "admin")]
impl IRequest<Vec<RoleModel>> for ListRolesRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct CreateRoleRequest {
    pub name: String,
    pub description: String,
}

#[post("/api/roles")]
#[authorize(role = "admin")]
impl IRequest<RoleModel> for CreateRoleRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct UpdateRoleRequest {
    #[serde(default)]
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
}

#[put("/api/roles/{id}")]
#[authorize(role = "admin")]
impl IRequest<RoleModel> for UpdateRoleRequest {}

#[claims]
#[derive(Default)]
pub struct DeleteRoleRequest {
    pub id: String,
}

#[delete("/api/roles/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteRoleRequest {}

// ── Role assignment ──

#[derive(Default, Deserialize)]
pub struct AssignRoleRequest {
    pub user_id: String,
    pub role_id: String,
}

#[post("/api/role-users")]
#[authorize(role = "admin")]
impl IRequest<String> for AssignRoleRequest {}

#[derive(Default)]
pub struct RevokeRoleRequest {
    pub user_id: String,
    pub role_id: String,
}

#[delete("/api/role-users/{user_id}/{role_id}")]
#[authorize(role = "admin")]
impl IRequest<String> for RevokeRoleRequest {}

// ── Resource CRUD (admin-maintained 通用资源) ──

#[derive(Default)]
pub struct ListResourcesRequest;

#[get("/api/resources")]
#[authorize(role = "admin")]
impl IRequest<Vec<ResourceModel>> for ListResourcesRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct CreateResourceRequest {
    pub name: String,
    pub description: String,
    pub r#type: String,
    pub value: String,
    pub properties: String,
}

#[post("/api/resources")]
#[authorize(role = "admin")]
impl IRequest<ResourceModel> for CreateResourceRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct UpdateResourceRequest {
    #[serde(default)]
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub r#type: Option<String>,
    pub value: Option<String>,
    pub properties: Option<String>,
}

#[put("/api/resources/{id}")]
#[authorize(role = "admin")]
impl IRequest<ResourceModel> for UpdateResourceRequest {}

#[claims]
#[derive(Default)]
pub struct DeleteResourceRequest {
    pub id: String,
}

#[delete("/api/resources/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteResourceRequest {}

// ── Authorize CRUD (role ↔ resource links) ──

#[derive(Default)]
pub struct ListAuthorizesRequest;

#[get("/api/authorizes")]
#[authorize(role = "admin")]
impl IRequest<Vec<AuthorizeModel>> for ListAuthorizesRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct CreateAuthorizeRequest {
    pub role_id: String,
    pub resource_id: String,
}

#[post("/api/authorizes")]
#[authorize(role = "admin")]
impl IRequest<AuthorizeModel> for CreateAuthorizeRequest {}

#[derive(Default)]
pub struct DeleteAuthorizeRequest {
    pub id: String,
}

#[delete("/api/authorizes/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteAuthorizeRequest {}
