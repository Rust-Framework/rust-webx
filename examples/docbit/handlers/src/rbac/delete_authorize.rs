//! `DELETE /api/authorizes/{id}` — remove a role ↔ resource link (admin).
//!
//! `authorize` is a join table: it is hard-deleted, not soft-deleted.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::rbac::DeleteAuthorizeRequest;
use docbit_domain::entities::Authorize;

use crate::common::parse_id;

#[derive(Inject)]
pub struct DeleteAuthorizeHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteAuthorizeRequest, String> for DeleteAuthorizeHandler {
    async fn handle(&mut self, req: DeleteAuthorizeRequest) -> ActionResult<String> {
        let id = parse_id(&req.id)?;
        let q = id.clone();

        let affected = linq!(self.ctx.set::<Authorize>(), |a: Authorize| a.id == q)
            .execute_delete()
            .await?;

        if affected > 0 {
            Ok(format!("Deleted authorize {}", id))
        } else {
            Err(Error::not_found("Authorize not found"))
        }
    }
}
