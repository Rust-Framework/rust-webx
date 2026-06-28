//! RBAC handlers — Role / Resource / Authorize / RoleUser CRUD.
//!
//! 主表（Role/Resource）用软删除；联结表（RoleUser/Authorize）用硬删除
//! （`linq!` 类型安全谓词 + `execute_delete` 直接 DB 删除，避免 load_all 三段式）。

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webapp::*;
use tokio::sync::Mutex;

use docbit_contracts::rbac::*;
use docbit_domain::entities::{Authorize, Resource, Role, RoleUser};
use docbit_domain::{ApplyTo, ToEntity, ToModel};

use crate::util::{now_secs, operator_id, parse_id};

// ── Role CRUD ──

#[derive(Inject)]
pub struct ListRolesHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct CreateRoleHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct UpdateRoleHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct DeleteRoleHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[inject]
#[async_trait]
impl IRequestHandler<ListRolesRequest, Vec<RoleModel>> for ListRolesHandler {
    async fn handle(&self, _: ListRolesRequest) -> Result<Vec<RoleModel>> {
        let roles = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Role>(), |r: Role| !r.is_deleted)
                .to_list()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        Ok(roles.into_iter().map(RoleModel::from).collect())
    }
}

#[inject]
#[async_trait]
impl IRequestHandler<CreateRoleRequest, RoleModel> for CreateRoleHandler {
    async fn handle(&self, req: CreateRoleRequest) -> Result<RoleModel> {
        let op = operator_id(req.claims.as_deref()).unwrap_or(0);
        let now = now_secs();
        let name = req.name.clone();
        let role = req.to_entity(op, now);
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Role>().add(role);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to create role: {}", e)))?;
        }
        let created = {
            let mut ctx = self.ctx.lock().await;
            let q = name.clone();
            linq!(ctx.set::<Role>(), |r: Role| r.name == q)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::Internal("Role disappeared after insert".into()))?;
        Ok(created.to_model())
    }
}

#[inject]
#[async_trait]
impl IRequestHandler<UpdateRoleRequest, RoleModel> for UpdateRoleHandler {
    async fn handle(&self, req: UpdateRoleRequest) -> Result<RoleModel> {
        let id = parse_id(&req.id)?;
        let mut role = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Role>(), |r: Role| r.id == id)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Role not found".into()))?;
        let op = operator_id(req.claims.as_deref()).unwrap_or(0);
        req.apply_to(&mut role, op, now_secs());
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Role>().update(role);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }
        let updated = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Role>(), |r: Role| r.id == id)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Role not found after update".into()))?;
        Ok(updated.to_model())
    }
}

#[inject]
#[async_trait]
impl IRequestHandler<DeleteRoleRequest, String> for DeleteRoleHandler {
    async fn handle(&self, req: DeleteRoleRequest) -> Result<String> {
        let id = parse_id(&req.id)?;
        let mut role = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Role>(), |r: Role| r.id == id)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Role not found".into()))?;
        role.is_deleted = true;
        role.updated_id = operator_id(req.claims.as_deref());
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

#[derive(Inject)]
pub struct AssignRoleHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct RevokeRoleHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[inject]
#[async_trait]
impl IRequestHandler<AssignRoleRequest, String> for AssignRoleHandler {
    async fn handle(&self, req: AssignRoleRequest) -> Result<String> {
        // 幂等：若已存在则跳过
        let exists = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<RoleUser>(), |r: RoleUser| r.user_id == req.user_id && r.role_id == req.role_id)
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

#[inject]
#[async_trait]
impl IRequestHandler<RevokeRoleRequest, String> for RevokeRoleHandler {
    async fn handle(&self, req: RevokeRoleRequest) -> Result<String> {
        let uid = parse_id(&req.user_id)?;
        let rid = parse_id(&req.role_id)?;
        // rust-ef 最佳实践：用 `linq!` 类型安全谓词 + `execute_delete` 直接 DB 删除，
        // 避免旧的 `load_all` + `tracked_entries` + `remove_at` + `save_changes` 三段式。
        let affected = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<RoleUser>(), |r: RoleUser| r.user_id == uid && r.role_id == rid)
                .execute_delete()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        if affected > 0 {
            Ok(format!("Revoked role {} from user {}", rid, uid))
        } else {
            Ok(format!("Role {} not assigned to user {}", rid, uid))
        }
    }
}

// ── Resource CRUD ──

#[derive(Inject)]
pub struct ListResourcesHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct CreateResourceHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct UpdateResourceHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct DeleteResourceHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[inject]
#[async_trait]
impl IRequestHandler<ListResourcesRequest, Vec<ResourceModel>> for ListResourcesHandler {
    async fn handle(&self, _: ListResourcesRequest) -> Result<Vec<ResourceModel>> {
        let items = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Resource>(), |r: Resource| !r.is_deleted)
                .to_list()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        Ok(items.into_iter().map(ResourceModel::from).collect())
    }
}

#[inject]
#[async_trait]
impl IRequestHandler<CreateResourceRequest, ResourceModel> for CreateResourceHandler {
    async fn handle(&self, req: CreateResourceRequest) -> Result<ResourceModel> {
        let op = operator_id(req.claims.as_deref()).unwrap_or(0);
        let now = now_secs();
        let value = req.value.clone();
        let res = req.to_entity(op, now);
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
            let q = value.clone();
            linq!(ctx.set::<Resource>(), |r: Resource| r.value == q)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::Internal("Resource disappeared after insert".into()))?;
        Ok(created.to_model())
    }
}

#[inject]
#[async_trait]
impl IRequestHandler<UpdateResourceRequest, ResourceModel> for UpdateResourceHandler {
    async fn handle(&self, req: UpdateResourceRequest) -> Result<ResourceModel> {
        let id = parse_id(&req.id)?;
        let mut res = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Resource>(), |r: Resource| r.id == id)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Resource not found".into()))?;
        let op = operator_id(req.claims.as_deref()).unwrap_or(0);
        req.apply_to(&mut res, op, now_secs());
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Resource>().update(res);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }
        let updated = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Resource>(), |r: Resource| r.id == id)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Resource not found after update".into()))?;
        Ok(updated.to_model())
    }
}

#[inject]
#[async_trait]
impl IRequestHandler<DeleteResourceRequest, String> for DeleteResourceHandler {
    async fn handle(&self, req: DeleteResourceRequest) -> Result<String> {
        let id = parse_id(&req.id)?;
        let mut res = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Resource>(), |r: Resource| r.id == id)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Resource not found".into()))?;
        res.is_deleted = true;
        res.updated_id = operator_id(req.claims.as_deref());
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

#[derive(Inject)]
pub struct ListAuthorizesHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct CreateAuthorizeHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct DeleteAuthorizeHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[inject]
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

#[inject]
#[async_trait]
impl IRequestHandler<CreateAuthorizeRequest, AuthorizeModel> for CreateAuthorizeHandler {
    async fn handle(&self, req: CreateAuthorizeRequest) -> Result<AuthorizeModel> {
        // 幂等：若已存在则返回现有
        let (role_id, resource_id) = (req.role_id, req.resource_id);
        let exists = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Authorize>(), |a: Authorize| a.role_id == role_id && a.resource_id == resource_id)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        if let Some(e) = exists {
            return Ok(e.to_model());
        }
        let now = now_secs();
        let authorize = req.to_entity(0, now);
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Authorize>().add(authorize);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to create authorize: {}", e)))?;
        }
        let created = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Authorize>(), |a: Authorize| a.role_id == role_id && a.resource_id == resource_id)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::Internal("Authorize disappeared after insert".into()))?;
        Ok(created.to_model())
    }
}

#[inject]
#[async_trait]
impl IRequestHandler<DeleteAuthorizeRequest, String> for DeleteAuthorizeHandler {
    async fn handle(&self, req: DeleteAuthorizeRequest) -> Result<String> {
        let id = parse_id(&req.id)?;
        // rust-ef 最佳实践：`linq!` 类型安全谓词 + `execute_delete` 直接 DB 删除，
        // 避免旧的 `load_all` + `tracked_entries` + `remove_at` + `save_changes` 三段式。
        let affected = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Authorize>(), |a: Authorize| a.id == id)
                .execute_delete()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        if affected > 0 {
            Ok(format!("Deleted authorize {}", id))
        } else {
            Err(Error::NotFound("Authorize not found".into()))
        }
    }
}
