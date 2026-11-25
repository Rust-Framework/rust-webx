//! `GET /api/products/{id}/specs` — the specs of one product.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use dmbit_contracts::spec::{ListProductSpecsRequest, SpecModel};
use dmbit_domain::entities::Spec;

use super::queries::{require_product_name, spec_models_for};
use crate::common::{parse_id, MAX_LIST_ROWS};

#[derive(Inject)]
pub struct ListProductSpecsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListProductSpecsRequest, Vec<SpecModel>> for ListProductSpecsHandler {
    async fn handle(&mut self, req: ListProductSpecsRequest) -> ActionResult<Vec<SpecModel>> {
        let product_id = parse_id(&req.id)?;
        let name = require_product_name(&mut self.ctx, &product_id).await?;

        let q = product_id.to_string();
        let specs = linq!(self.ctx.set::<Spec>(), |s: Spec| s.product_id == q)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;
        let refs: Vec<&Spec> = specs.iter().collect();
        spec_models_for(&mut self.ctx, &refs, &name).await
    }
}
