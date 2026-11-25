//! `GET /api/users` — every registered user, newest first (admin).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::user::{ListUsersRequest, UserModel};
use docbit_domain::entities::User;

use crate::common::MAX_LIST_ROWS;

#[derive(Inject)]
pub struct ListUsersHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListUsersRequest, Vec<UserModel>> for ListUsersHandler {
    async fn handle(&mut self, _: ListUsersRequest) -> ActionResult<Vec<UserModel>> {
        let users = linq!(self.ctx.set::<User>(); include u.roles; order_by u.created_at desc)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;

        Ok(users.iter().map(UserModel::from).collect())
    }
}
