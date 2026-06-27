//! RBAC contracts — roles, resources (route patterns), and authorizations.

use rust_webapp::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleModel {
    pub id: i32,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceModel {
    pub id: i32,
    pub route_pattern: String,
    pub method: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizeModel {
    pub id: i32,
    pub role_id: i32,
    pub resource_id: i32,
}

// ── Role CRUD ──

pub struct ListRolesRequest;

#[get("/api/roles")]
#[authorize(role = "admin")]
impl IRequest<Vec<RoleModel>> for ListRolesRequest {}

#[derive(Deserialize)]
pub struct CreateRoleRequest {
    pub name: String,
    pub description: String,
}

#[post("/api/roles")]
#[authorize(role = "admin")]
impl IRequest<RoleModel> for CreateRoleRequest {}

pub struct DeleteRoleRequest {
    pub id: String,
}

#[delete("/api/roles/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteRoleRequest {}

// ── Role assignment ──

#[derive(Deserialize)]
pub struct AssignRoleRequest {
    pub user_id: i32,
    pub role_id: i32,
}

#[post("/api/role-users")]
#[authorize(role = "admin")]
impl IRequest<String> for AssignRoleRequest {}

pub struct RevokeRoleRequest {
    pub user_id: String,
    pub role_id: String,
}

#[delete("/api/role-users/{user_id}/{role_id}")]
#[authorize(role = "admin")]
impl IRequest<String> for RevokeRoleRequest {}

// ── Resource CRUD (admin-maintained route patterns) ──

pub struct ListResourcesRequest;

#[get("/api/resources")]
#[authorize(role = "admin")]
impl IRequest<Vec<ResourceModel>> for ListResourcesRequest {}

#[derive(Deserialize)]
pub struct CreateResourceRequest {
    pub route_pattern: String,
    pub method: String,
    pub description: String,
}

#[post("/api/resources")]
#[authorize(role = "admin")]
impl IRequest<ResourceModel> for CreateResourceRequest {}

pub struct DeleteResourceRequest {
    pub id: String,
}

#[delete("/api/resources/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteResourceRequest {}

// ── Authorize CRUD (role ↔ resource links) ──

pub struct ListAuthorizesRequest;

#[get("/api/authorizes")]
#[authorize(role = "admin")]
impl IRequest<Vec<AuthorizeModel>> for ListAuthorizesRequest {}

#[derive(Deserialize)]
pub struct CreateAuthorizeRequest {
    pub role_id: i32,
    pub resource_id: i32,
}

#[post("/api/authorizes")]
#[authorize(role = "admin")]
impl IRequest<AuthorizeModel> for CreateAuthorizeRequest {}

pub struct DeleteAuthorizeRequest {
    pub id: String,
}

#[delete("/api/authorizes/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteAuthorizeRequest {}
