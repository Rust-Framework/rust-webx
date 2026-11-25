//! `GET /api/authorizes` — every role ↔ resource link (admin).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::rbac::{AuthorizeModel, ListAuthorizesRequest};
use docbit_domain::entities::Authorize;

use crate::common::MAX_LIST_ROWS;

#[derive(Inject)]
pub struct ListAuthorizesHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListAuthorizesRequest, Vec<AuthorizeModel>> for ListAuthorizesHandler {
    async fn handle(&mut self, _: ListAuthorizesRequest) -> ActionResult<Vec<AuthorizeModel>> {
        let items = linq!(self.ctx.set::<Authorize>();)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;

        Ok(items.iter().map(AuthorizeModel::from).collect())
    }
}
