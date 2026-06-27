//! RBAC handlers — Role / Resource / Authorize / RoleUser CRUD.
//!
//! 主表（Role/Resource）用软删除；联结表（RoleUser/Authorize）用硬删除
//! （通过 `load_all` + `remove_at` + `save_changes`）。

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*, provider::DbValue};
use rust_webapp::*;
use tokio::sync::Mutex;

use docbit_contracts::rbac::*;
use docbit_domain::entities::{Authorize, Resource, Role, RoleUser};

use crate::util::{now_secs, operator_id, parse_id};

// ── Role CRUD ──

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListRolesRequest, Vec<RoleModel>>)]
pub struct ListRolesHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<CreateRoleRequest, RoleModel>)]
pub struct CreateRoleHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<UpdateRoleRequest, RoleModel>)]
pub struct UpdateRoleHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<DeleteRoleRequest, String>)]
pub struct DeleteRoleHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListRolesRequest, Vec<RoleModel>> for ListRolesHandler {
    async fn handle(&self, _: ListRolesRequest) -> Result<Vec<RoleModel>> {
        let roles = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Role>()
                .query()
                .filter_column("is_deleted", "=", DbValue::Bool(false))
                .to_list()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        Ok(roles.into_iter().map(RoleModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateRoleRequest, RoleModel> for CreateRoleHandler {
    async fn handle(&self, _: CreateRoleRequest) -> Result<RoleModel> {
        unreachable!("handle_with_claims is always called")
    }
    async fn handle_with_claims(
        &self,
        req: CreateRoleRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<RoleModel> {
        let op = operator_id(claims);
        let now = now_secs();
        let role = Role {
            id: 0,
            name: req.name.clone(),
            description: req.description.clone(),
            created_id: op,
            created_at: now,
            updated_id: op,
            updated_at: now,
            is_deleted: false,
            users: HasMany::new(),
            resources: HasMany::new(),
        };
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Role>().add(role);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to create role: {}", e)))?;
        }
        let created = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Role>()
                .query()
                .filter_column("name", "=", DbValue::String(req.name.clone()))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::Internal("Role disappeared after insert".into()))?;
        Ok(RoleModel::from(created))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateRoleRequest, RoleModel> for UpdateRoleHandler {
    async fn handle(&self, _: UpdateRoleRequest) -> Result<RoleModel> {
        unreachable!("handle_with_claims is always called")
    }
    async fn handle_with_claims(
        &self,
        req: UpdateRoleRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<RoleModel> {
        let id = parse_id(&req.id)?;
        let mut role = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Role>()
                .query()
                .filter_column("id", "=", DbValue::I32(id))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Role not found".into()))?;
        if let Some(n) = req.name {
            role.name = n;
        }
        if let Some(d) = req.description {
            role.description = d;
        }
        role.updated_id = operator_id(claims);
        role.updated_at = now_secs();
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Role>().update(role);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }
        let updated = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Role>()
                .query()
                .filter_column("id", "=", DbValue::I32(id))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Role not found after update".into()))?;
        Ok(RoleModel::from(updated))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteRoleRequest, String> for DeleteRoleHandler {
    async fn handle(&self, _: DeleteRoleRequest) -> Result<String> {
        unreachable!("handle_with_claims is always called")
    }
    async fn handle_with_claims(
        &self,
        req: DeleteRoleRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<String> {
        let id = parse_id(&req.id)?;
        let mut role = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Role>()
                .query()
                .filter_column("id", "=", DbValue::I32(id))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Role not found".into()))?;
        role.is_deleted = true;
        role.updated_id = operator_id(claims);
        role.updated_at = now_secs();
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Role>().update(role);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }
        Ok(format!("Deleted role {}", id))
    }
}

// ── Role assignment (RoleUser join) ──

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<AssignRoleRequest, String>)]
pub struct AssignRoleHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<RevokeRoleRequest, String>)]
pub struct RevokeRoleHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<AssignRoleRequest, String> for AssignRoleHandler {
    async fn handle(&self, req: AssignRoleRequest) -> Result<String> {
        // 幂等：若已存在则跳过
        let exists = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<RoleUser>()
                .query()
                .filter_column("user_id", "=", DbValue::I32(req.user_id))
                .filter_column("role_id", "=", DbValue::I32(req.role_id))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        if exists.is_some() {
            return Ok(format!("Role {} already assigned to user {}", req.role_id, req.user_id));
        }
        let now = now_secs();
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<RoleUser>().add(RoleUser {
                id: 0,
                user_id: req.user_id,
                role_id: req.role_id,
                created_at: now,
            });
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to assign role: {}", e)))?;
        }
        Ok(format!("Assigned role {} to user {}", req.role_id, req.user_id))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<RevokeRoleRequest, String> for RevokeRoleHandler {
    async fn handle(&self, req: RevokeRoleRequest) -> Result<String> {
        let uid = parse_id(&req.user_id)?;
        let rid = parse_id(&req.role_id)?;
        {
            let mut ctx = self.ctx.lock().await;
            let set = ctx.set::<RoleUser>();
            set.load_all()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
            let idx = set
                .tracked_entries()
                .position(|r| r.user_id == uid && r.role_id == rid);
            if let Some(i) = idx {
                set.remove_at(i).map_err(|e| Error::Internal(e.to_string()))?;
                ctx.save_changes()
                    .await
                    .map_err(|e| Error::Internal(e.to_string()))?;
                Ok(format!("Revoked role {} from user {}", rid, uid))
            } else {
                Ok(format!("Role {} not assigned to user {}", rid, uid))
            }
        }
    }
}

// ── Resource CRUD ──

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListResourcesRequest, Vec<ResourceModel>>)]
pub struct ListResourcesHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<CreateResourceRequest, ResourceModel>)]
pub struct CreateResourceHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<UpdateResourceRequest, ResourceModel>)]
pub struct UpdateResourceHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<DeleteResourceRequest, String>)]
pub struct DeleteResourceHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListResourcesRequest, Vec<ResourceModel>> for ListResourcesHandler {
    async fn handle(&self, _: ListResourcesRequest) -> Result<Vec<ResourceModel>> {
        let items = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Resource>()
                .query()
                .filter_column("is_deleted", "=", DbValue::Bool(false))
                .to_list()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        Ok(items.into_iter().map(ResourceModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateResourceRequest, ResourceModel> for CreateResourceHandler {
    async fn handle(&self, _: CreateResourceRequest) -> Result<ResourceModel> {
        unreachable!("handle_with_claims is always called")
    }
    async fn handle_with_claims(
        &self,
        req: CreateResourceRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<ResourceModel> {
        let op = operator_id(claims);
        let now = now_secs();
        let res = Resource {
            id: 0,
            name: req.name.clone(),
            description: req.description.clone(),
            resource_type: req.r#type.clone(),
            value: req.value.clone(),
            properties: req.properties.clone(),
            created_id: op,
            created_at: now,
            updated_id: op,
            updated_at: now,
            is_deleted: false,
            roles: HasMany::new(),
        };
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Resource>().add(res);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to create resource: {}", e)))?;
        }
        // 回查（name 可能重复，按 value 过滤更精确）
        let created = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Resource>()
                .query()
                .filter_column("value", "=", DbValue::String(req.value.clone()))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::Internal("Resource disappeared after insert".into()))?;
        Ok(ResourceModel::from(created))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateResourceRequest, ResourceModel> for UpdateResourceHandler {
    async fn handle(&self, _: UpdateResourceRequest) -> Result<ResourceModel> {
        unreachable!("handle_with_claims is always called")
    }
    async fn handle_with_claims(
        &self,
        req: UpdateResourceRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<ResourceModel> {
        let id = parse_id(&req.id)?;
        let mut res = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Resource>()
                .query()
                .filter_column("id", "=", DbValue::I32(id))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Resource not found".into()))?;
        if let Some(n) = req.name {
            res.name = n;
        }
        if let Some(d) = req.description {
            res.description = d;
        }
        if let Some(t) = req.r#type {
            res.resource_type = t;
        }
        if let Some(v) = req.value {
            res.value = v;
        }
        if let Some(p) = req.properties {
            res.properties = p;
        }
        res.updated_id = operator_id(claims);
        res.updated_at = now_secs();
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Resource>().update(res);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }
        let updated = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Resource>()
                .query()
                .filter_column("id", "=", DbValue::I32(id))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Resource not found after update".into()))?;
        Ok(ResourceModel::from(updated))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteResourceRequest, String> for DeleteResourceHandler {
    async fn handle(&self, _: DeleteResourceRequest) -> Result<String> {
        unreachable!("handle_with_claims is always called")
    }
    async fn handle_with_claims(
        &self,
        req: DeleteResourceRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<String> {
        let id = parse_id(&req.id)?;
        let mut res = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Resource>()
                .query()
                .filter_column("id", "=", DbValue::I32(id))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Resource not found".into()))?;
        res.is_deleted = true;
        res.updated_id = operator_id(claims);
        res.updated_at = now_secs();
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Resource>().update(res);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }
        Ok(format!("Deleted resource {}", id))
    }
}

// ── Authorize CRUD (role ↔ resource links) ──

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListAuthorizesRequest, Vec<AuthorizeModel>>)]
pub struct ListAuthorizesHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<CreateAuthorizeRequest, AuthorizeModel>)]
pub struct CreateAuthorizeHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<DeleteAuthorizeRequest, String>)]
pub struct DeleteAuthorizeHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListAuthorizesRequest, Vec<AuthorizeModel>> for ListAuthorizesHandler {
    async fn handle(&self, _: ListAuthorizesRequest) -> Result<Vec<AuthorizeModel>> {
        let items = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Authorize>().query().to_list().await.map_err(|e| Error::Internal(e.to_string()))?
        };
        Ok(items.into_iter().map(AuthorizeModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateAuthorizeRequest, AuthorizeModel> for CreateAuthorizeHandler {
    async fn handle(&self, req: CreateAuthorizeRequest) -> Result<AuthorizeModel> {
        // 幂等：若已存在则返回现有
        let exists = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Authorize>()
                .query()
                .filter_column("role_id", "=", DbValue::I32(req.role_id))
                .filter_column("resource_id", "=", DbValue::I32(req.resource_id))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        if let Some(e) = exists {
            return Ok(AuthorizeModel::from(e));
        }
        let now = now_secs();
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Authorize>().add(Authorize {
                id: 0,
                role_id: req.role_id,
                resource_id: req.resource_id,
                created_at: now,
            });
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to create authorize: {}", e)))?;
        }
        let created = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Authorize>()
                .query()
                .filter_column("role_id", "=", DbValue::I32(req.role_id))
                .filter_column("resource_id", "=", DbValue::I32(req.resource_id))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::Internal("Authorize disappeared after insert".into()))?;
        Ok(AuthorizeModel::from(created))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteAuthorizeRequest, String> for DeleteAuthorizeHandler {
    async fn handle(&self, req: DeleteAuthorizeRequest) -> Result<String> {
        let id = parse_id(&req.id)?;
        {
            let mut ctx = self.ctx.lock().await;
            let set = ctx.set::<Authorize>();
            set.load_all()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
            let idx = set.tracked_entries().position(|a| a.id == id);
            if let Some(i) = idx {
                set.remove_at(i).map_err(|e| Error::Internal(e.to_string()))?;
                ctx.save_changes()
                    .await
                    .map_err(|e| Error::Internal(e.to_string()))?;
                Ok(format!("Deleted authorize {}", id))
            } else {
                Err(Error::NotFound("Authorize not found".into()))
            }
        }
    }
}
