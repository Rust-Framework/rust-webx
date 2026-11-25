//! `GET /api/auth/me` — the current principal's profile.

use rust_ef::db_context::DbContext;
use webx::*;

use docbit_contracts::auth::{AuthMeRequest, UserModel};

use crate::common::operator_id;

use super::queries::load_user_by_id;

#[derive(Inject)]
pub struct AuthMeHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<AuthMeRequest, UserModel> for AuthMeHandler {
    async fn handle(&mut self, _req: AuthMeRequest) -> ActionResult<UserModel> {
        // ── 身份：sub 只来自 RequestContext（鉴权中间件写入） ──
        let uid = operator_id().ok_or_else(|| Error::unauthorized("Not authenticated"))?;

        // ── 加载 ──
        let user = load_user_by_id(&mut self.ctx, &uid)
            .await?
            .ok_or_else(|| Error::not_found("User not found"))?;

        Ok(UserModel::from(&user))
    }
}
