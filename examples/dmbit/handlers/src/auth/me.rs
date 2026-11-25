//! `GET /api/auth/me` — the current principal.

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::auth::{AuthMeRequest, UserModel};

use super::queries::user_by_id;

#[derive(Inject)]
pub struct AuthMeHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<AuthMeRequest, UserModel> for AuthMeHandler {
    async fn handle(&mut self, req: AuthMeRequest) -> ActionResult<UserModel> {
        // Identity comes from the token field, never from the body/query.
        let uid = req.require_user()?.id.clone();
        let user = user_by_id(&mut self.ctx, &uid)
            .await?
            .ok_or_else(|| Error::not_found("用户不存在"))?;
        Ok(UserModel::from(&user))
    }
}
