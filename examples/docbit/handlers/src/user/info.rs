//! `GET /api/info` — API server information.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::user::InfoRequest;
use docbit_domain::entities::User;

#[derive(Inject)]
pub struct InfoHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<InfoRequest, String> for InfoHandler {
    async fn handle(&mut self, _: InfoRequest) -> ActionResult<String> {
        let count: i64 = linq!(self.ctx.set::<User>(); count).await?;

        Ok(format!("Total users: {}", count))
    }
}
