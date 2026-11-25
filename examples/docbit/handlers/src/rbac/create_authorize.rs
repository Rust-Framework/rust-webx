//! `POST /api/authorizes` — link a resource to a role (admin, idempotent).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::rbac::{AuthorizeModel, CreateAuthorizeRequest};
use docbit_domain::entities::Authorize;

use crate::common::save_changes;

#[derive(Inject)]
pub struct CreateAuthorizeHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateAuthorizeRequest, AuthorizeModel> for CreateAuthorizeHandler {
    async fn handle(&mut self, req: CreateAuthorizeRequest) -> ActionResult<AuthorizeModel> {
        let (role_id, resource_id) = (req.role_id.clone(), req.resource_id.clone());

        // ── 校验：已有同一链接则直接返回 ──
        let (rid, sid) = (role_id.clone(), resource_id.clone());
        let exists = linq!(self.ctx.set::<Authorize>(), |a: Authorize| a.role_id == rid
            && a.resource_id == sid)
        .first_or_default()
        .await?;

        if let Some(existing) = exists {
            return Ok(AuthorizeModel::from(&existing));
        }

        // ── 变更 ──
        let entity = Authorize::try_from(req)?;
        let model = AuthorizeModel::from(&entity);
        self.ctx.add(entity);

        save_changes(&mut self.ctx).await?;

        Ok(model)
    }
}
