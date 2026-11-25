//! `GET /api/users/{id}` — one user, with roles (admin).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::user::{GetUserRequest, UserModel};
use docbit_domain::entities::User;

use crate::common::parse_id;

#[derive(Inject)]
pub struct GetUserHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetUserRequest, UserModel> for GetUserHandler {
    async fn handle(&mut self, req: GetUserRequest) -> ActionResult<UserModel> {
        let q = parse_id(&req.id)?;

        let user = linq!(self.ctx.set::<User>(), |u: User| u.id == q; include u.roles)
            .first_or_default()
            .await?
            .ok_or_else(|| Error::not_found("User not found"))?;

        Ok(UserModel::from(&user))
    }
}
