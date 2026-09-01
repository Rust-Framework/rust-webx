//! RBAC handlers — Role / Resource / Authorize / RoleUser CRUD.
//!
//! 主表（Role/Resource）用软删除；联结表（RoleUser/Authorize）用硬删除
//! （`linq!` 类型安全谓词 + `execute_delete` 直接 DB 删除）。
//!
//! 每个 handler 持有 owned `DbContext`，`handle(&mut self, ...)` 直接操作 `self.ctx`。

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use docbit_contracts::rbac::*;
use docbit_domain::entities::{Authorize, Resource, Role, RoleUser};
use docbit_domain::{new_id, ApplyTo, ToEntity, ToModel};

use crate::db::{save_changes, EfResultExt};
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
        let roles = linq!(self.ctx.set::<Role>();)
            .to_list()
            .await
            .map_ef()?;

        Ok(roles.into_iter().map(RoleModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateRoleRequest, RoleModel> for CreateRoleHandler {
    async fn handle(&mut self, req: CreateRoleRequest) -> Result<RoleModel> {
        let now = now_secs();
        let id = new_id();

        let entity = req.to_entity(id, now);

        self.ctx.add(entity.clone());

        save_changes(&mut self.ctx).await?;

        Ok(entity.to_model())
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
            .find(id.clone())
            .await
            .map_ef()?
            .ok_or_else(|| Error::NotFound("Role not found".into()))?;

        req.apply_to(&mut role, now_secs());

        self.ctx.update(role.clone());

        save_changes(&mut self.ctx).await?;

        Ok(role.to_model())
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
            .find(id.clone())
            .await
            .map_ef()?
            .ok_or_else(|| Error::NotFound("Role not found".into()))?;

        role.is_deleted = true;
        role.updated_id = operator_id();
        role.updated_at = now_secs();

        self.ctx.update(role);

        save_changes(&mut self.ctx).await?;

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
        let user_id = req.user_id.clone();
        let role_id = req.role_id.clone();
        let exists_uid = user_id.clone();
        let exists_rid = role_id.clone();

        let exists = linq!(self.ctx.set::<RoleUser>(), |r: RoleUser| r.user_id == exists_uid && r.role_id == exists_rid)
            .first_or_default()
            .await
            .map_ef()?;

        if exists.is_some() {
            return Ok(format!(
                "Role {} already assigned to user {}",
                role_id, user_id
            ));
        }

        let now = now_secs();
        let entity = RoleUser {
            id: new_id(),
            user_id: user_id.clone(),
            role_id: role_id.clone(),
            created_at: now,
        };

        self.ctx.add(entity);

        save_changes(&mut self.ctx).await?;

        Ok(format!("Assigned role {} to user {}", role_id, user_id))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<RevokeRoleRequest, String> for RevokeRoleHandler {
    async fn handle(&mut self, req: RevokeRoleRequest) -> Result<String> {
        let uid = parse_id(&req.user_id)?;
        let rid = parse_id(&req.role_id)?;
        let uid_q = uid.clone();
        let rid_q = rid.clone();

        let affected = linq!(self.ctx.set::<RoleUser>(), |r: RoleUser| r.user_id == uid_q && r.role_id == rid_q)
            .execute_delete()
            .await
            .map_ef()?;

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
        let items = linq!(self.ctx.set::<Resource>();)
            .to_list()
            .await
            .map_ef()?;

        Ok(items.into_iter().map(ResourceModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateResourceRequest, ResourceModel> for CreateResourceHandler {
    async fn handle(&mut self, req: CreateResourceRequest) -> Result<ResourceModel> {
        let now = now_secs();
        let id = new_id();

        let entity = req.to_entity(id, now);

        self.ctx.add(entity.clone());

        save_changes(&mut self.ctx).await?;

        Ok(entity.to_model())
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
            .find(id.clone())
            .await
            .map_ef()?
            .ok_or_else(|| Error::NotFound("Resource not found".into()))?;

        req.apply_to(&mut res, now_secs());

        self.ctx.update(res.clone());

        save_changes(&mut self.ctx).await?;

        Ok(res.to_model())
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
            .find(id.clone())
            .await
            .map_ef()?
            .ok_or_else(|| Error::NotFound("Resource not found".into()))?;

        res.is_deleted = true;
        res.updated_id = operator_id();
        res.updated_at = now_secs();

        self.ctx.update(res);

        save_changes(&mut self.ctx).await?;

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
            .map_ef()?;

        Ok(items.into_iter().map(AuthorizeModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateAuthorizeRequest, AuthorizeModel> for CreateAuthorizeHandler {
    async fn handle(&mut self, req: CreateAuthorizeRequest) -> Result<AuthorizeModel> {
        let role_id = req.role_id.clone();
        let resource_id = req.resource_id.clone();
        let exists_role = role_id.clone();
        let exists_resource = resource_id.clone();

        let exists = linq!(self.ctx.set::<Authorize>(), |a: Authorize| a.role_id == exists_role && a.resource_id == exists_resource)
            .first_or_default()
            .await
            .map_ef()?;

        if let Some(existing) = exists {
            return Ok(existing.to_model());
        }

        let now = now_secs();
        let id = new_id();

        let entity = req.to_entity(id, now);

        self.ctx.add(entity.clone());

        save_changes(&mut self.ctx).await?;

        Ok(entity.to_model())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteAuthorizeRequest, String> for DeleteAuthorizeHandler {
    async fn handle(&mut self, req: DeleteAuthorizeRequest) -> Result<String> {
        let id = parse_id(&req.id)?;
        let q = id.clone();

        let affected = linq!(self.ctx.set::<Authorize>(), |a: Authorize| a.id == q)
            .execute_delete()
            .await
            .map_ef()?;

        if affected > 0 {
            Ok(format!("Deleted authorize {}", id))
        } else {
            Err(Error::NotFound("Authorize not found".into()))
        }
    }
}
