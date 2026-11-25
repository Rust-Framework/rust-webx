//! `GET /api/specs/{id}` — one spec with its components.

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::spec::{GetSpecRequest, SpecModel};

use super::queries::{require_spec, spec_model_for};
use crate::common::parse_id;

#[derive(Inject)]
pub struct GetSpecHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetSpecRequest, SpecModel> for GetSpecHandler {
    async fn handle(&mut self, req: GetSpecRequest) -> ActionResult<SpecModel> {
        let id = parse_id(&req.id)?;
        let spec = require_spec(&mut self.ctx, &id).await?;
        let name = super::queries::require_product_name(&mut self.ctx, &spec.product_id).await?;
        spec_model_for(&mut self.ctx, &spec, &name).await
    }
}
