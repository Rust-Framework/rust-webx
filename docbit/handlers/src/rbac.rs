//! RBAC handlers — Role / Resource / Authorize / RoleUser CRUD.
//!
//! 主表（Role/Resource）用软删除；联结表（RoleUser/Authorize）用硬删除
//! （`linq!` 类型安全谓词 + `execute_delete` 直接 DB 删除，避免 load_all 三段式）。
//!
//! 每个 handler 持有 owned `DbContext`，`handle(&mut self, ...)` 直接操作 `self.ctx`。

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use docbit_contracts::rbac::*;
use docbit_domain::entities::{Authorize, Resource, Role, RoleUser};
use docbit_domain::{ApplyTo, ToEntity, ToModel};

use crate::util::{now_secs, operator_id, parse_id};

// ── Role CRUD ──

#[derive(Inject)]
pub struct ListRolesHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct CreateRoleHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct UpdateRoleHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct DeleteRoleHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListRolesRequest, Vec<RoleModel>> for ListRolesHandler {
    async fn handle(&mut self, _: ListRolesRequest) -> Result<Vec<RoleModel>> {
        let roles = linq!(self.ctx.set::<Role>(), |r: Role| !r.is_deleted)
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(roles.into_iter().map(RoleModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateRoleRequest, RoleModel> for CreateRoleHandler {
    async fn handle(&mut self, req: CreateRoleRequest) -> Result<RoleModel> {
        let op = operator_id(req.claims.as_deref()).unwrap_or(0);
        let now = now_secs();
        let name = req.name.clone();
        let role = req.to_entity(op, now);
        self.ctx.set::<Role>().add(role);
        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(format!("Failed to create role: {}", e)))?;
        // FIXME(framework): rust-ef 1.3.0 save_changes 不回填自增 id，按 name 回查。
        let created = {
            let q = name.clone();
            linq!(self.ctx.set::<Role>(), |r: Role| r.name == q && !r.is_deleted)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::Internal("Role disappeared after insert".into()))?;
        Ok(created.to_model())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateRoleRequest, RoleModel> for UpdateRoleHandler {
    async fn handle(&mut self, req: UpdateRoleRequest) -> Result<RoleModel> {
        let id = parse_id(&req.id)?;
        let mut role = self
            .ctx
            .set::<Role>()
            .query()
            .find(id)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Role not found".into()))?;
        let op = operator_id(req.claims.as_deref()).unwrap_or(0);
        req.apply_to(&mut role, op, now_secs());
        self.ctx.set::<Role>().update(role);
        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        let updated = self
            .ctx
            .set::<Role>()
            .query()
            .find(id)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Role not found after update".into()))?;
        Ok(updated.to_model())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteRoleRequest, String> for DeleteRoleHandler {
    async fn handle(&mut self, req: DeleteRoleRequest) -> Result<String> {
        let id = parse_id(&req.id)?;
        let mut role = self
            .ctx
            .set::<Role>()
            .query()
            .find(id)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Role not found".into()))?;
        role.is_deleted = true;
        role.updated_id = operator_id(req.claims.as_deref());
        role.updated_at = now_secs();
        self.ctx.set::<Role>().update(role);
        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(format!("Deleted role {}", id))
    }
}

// ── Role assignment (RoleUser join) ──

#[derive(Inject)]
pub struct AssignRoleHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct RevokeRoleHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<AssignRoleRequest, String> for AssignRoleHandler {
    async fn handle(&mut self, req: AssignRoleRequest) -> Result<String> {
        // 幂等：若已存在则跳过
        let exists = linq!(self.ctx.set::<RoleUser>(), |r: RoleUser| r.user_id == req.user_id && r.role_id == req.role_id)
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        if exists.is_some() {
            return Ok(format!("Role {} already assigned to user {}", req.role_id, req.user_id));
        }
        let now = now_secs();
        self.ctx.set::<RoleUser>().add(RoleUser {
            id: 0,
            user_id: req.user_id,
            role_id: req.role_id,
            created_at: now,
        });
        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(format!("Failed to assign role: {}", e)))?;
        Ok(format!("Assigned role {} to user {}", req.role_id, req.user_id))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<RevokeRoleRequest, String> for RevokeRoleHandler {
    async fn handle(&mut self, req: RevokeRoleRequest) -> Result<String> {
        let uid = parse_id(&req.user_id)?;
        let rid = parse_id(&req.role_id)?;
        // rust-ef 最佳实践：用 `linq!` 类型安全谓词 + `execute_delete` 直接 DB 删除，
        // 避免旧的 `load_all` + `tracked_entries` + `remove_at` + `save_changes` 三段式。
        let affected = linq!(self.ctx.set::<RoleUser>(), |r: RoleUser| r.user_id == uid && r.role_id == rid)
            .execute_delete()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
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
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct CreateResourceHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct UpdateResourceHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct DeleteResourceHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListResourcesRequest, Vec<ResourceModel>> for ListResourcesHandler {
    async fn handle(&mut self, _: ListResourcesRequest) -> Result<Vec<ResourceModel>> {
        let items = linq!(self.ctx.set::<Resource>(), |r: Resource| !r.is_deleted)
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(items.into_iter().map(ResourceModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateResourceRequest, ResourceModel> for CreateResourceHandler {
    async fn handle(&mut self, req: CreateResourceRequest) -> Result<ResourceModel> {
        let op = operator_id(req.claims.as_deref()).unwrap_or(0);
        let now = now_secs();
        let value = req.value.clone();
        let res = req.to_entity(op, now);
        // FIXME(framework): rust-ef 1.3.0 不回填自增 id，按 value 回查。
        // `Resource.value` 无 `#[unique]`，并发下可能取到他人记录。
        // 长期修复：框架暴露 last-insert-id + 为 `value` 加 `#[unique]`。
        self.ctx.set::<Resource>().add(res);
        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(format!("Failed to create resource: {}", e)))?;
        let created = {
            let q = value.clone();
            linq!(self.ctx.set::<Resource>(), |r: Resource| r.value == q)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
                .ok_or_else(|| Error::Internal("Resource disappeared after insert".into()))?
        };
        Ok(created.to_model())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateResourceRequest, ResourceModel> for UpdateResourceHandler {
    async fn handle(&mut self, req: UpdateResourceRequest) -> Result<ResourceModel> {
        let id = parse_id(&req.id)?;
        let mut res = self
            .ctx
            .set::<Resource>()
            .query()
            .find(id)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Resource not found".into()))?;
        let op = operator_id(req.claims.as_deref()).unwrap_or(0);
        req.apply_to(&mut res, op, now_secs());
        self.ctx.set::<Resource>().update(res);
        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        let updated = self
            .ctx
            .set::<Resource>()
            .query()
            .find(id)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Resource not found after update".into()))?;
        Ok(updated.to_model())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteResourceRequest, String> for DeleteResourceHandler {
    async fn handle(&mut self, req: DeleteResourceRequest) -> Result<String> {
        let id = parse_id(&req.id)?;
        let mut res = self
            .ctx
            .set::<Resource>()
            .query()
            .find(id)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Resource not found".into()))?;
        res.is_deleted = true;
        res.updated_id = operator_id(req.claims.as_deref());
        res.updated_at = now_secs();
        self.ctx.set::<Resource>().update(res);
        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(format!("Deleted resource {}", id))
    }
}

// ── Authorize CRUD (role ↔ resource links) ──

#[derive(Inject)]
pub struct ListAuthorizesHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct CreateAuthorizeHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct DeleteAuthorizeHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListAuthorizesRequest, Vec<AuthorizeModel>> for ListAuthorizesHandler {
    async fn handle(&mut self, _: ListAuthorizesRequest) -> Result<Vec<AuthorizeModel>> {
        let items = self
            .ctx
            .set::<Authorize>()
            .query()
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(items.into_iter().map(AuthorizeModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateAuthorizeRequest, AuthorizeModel> for CreateAuthorizeHandler {
    async fn handle(&mut self, req: CreateAuthorizeRequest) -> Result<AuthorizeModel> {
        // 幂等：若已存在则返回现有
        let (role_id, resource_id) = (req.role_id, req.resource_id);
        let exists = linq!(self.ctx.set::<Authorize>(), |a: Authorize| a.role_id == role_id && a.resource_id == resource_id)
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        if let Some(e) = exists {
            return Ok(e.to_model());
        }
        let now = now_secs();
        let authorize = req.to_entity(0, now);
        self.ctx.set::<Authorize>().add(authorize);
        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(format!("Failed to create authorize: {}", e)))?;
        let created = linq!(self.ctx.set::<Authorize>(), |a: Authorize| a.role_id == role_id && a.resource_id == resource_id)
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::Internal("Authorize disappeared after insert".into()))?;
        Ok(created.to_model())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteAuthorizeRequest, String> for DeleteAuthorizeHandler {
    async fn handle(&mut self, req: DeleteAuthorizeRequest) -> Result<String> {
        let id = parse_id(&req.id)?;
        // rust-ef 最佳实践：`linq!` 类型安全谓词 + `execute_delete` 直接 DB 删除，
        // 避免旧的 `load_all` + `tracked_entries` + `remove_at` + `save_changes` 三段式。
        let affected = linq!(self.ctx.set::<Authorize>(), |a: Authorize| a.id == id)
            .execute_delete()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        if affected > 0 {
            Ok(format!("Deleted authorize {}", id))
        } else {
            Err(Error::NotFound("Authorize not found".into()))
        }
    }
}
