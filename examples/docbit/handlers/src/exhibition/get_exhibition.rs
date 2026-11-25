//! `GET /api/exhibitions/{slug}` — one portfolio work.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::exhibition::{ExhibitionModel, GetExhibitionRequest};
use docbit_domain::entities::Exhibition;

use crate::common::parse_id;

#[derive(Inject)]
pub struct GetExhibitionHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetExhibitionRequest, ExhibitionModel> for GetExhibitionHandler {
    async fn handle(&mut self, req: GetExhibitionRequest) -> ActionResult<ExhibitionModel> {
        let q = parse_id(&req.slug)?;

        let item =
            linq!(self.ctx.set::<Exhibition>(), |e: Exhibition| e.slug == q; include e.category)
                .first_or_default()
                .await?
                .ok_or_else(|| Error::not_found(format!("Exhibition not found: {}", req.slug)))?;

        Ok(ExhibitionModel::from(&item))
    }
}
