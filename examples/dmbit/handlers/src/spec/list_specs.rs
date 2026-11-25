//! `GET /api/specs` — every spec.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use dmbit_contracts::spec::{ListSpecsRequest, SpecModel};
use dmbit_domain::entities::{Product, Spec};
use dmbit_domain::mappers::spec_mapper::spec_model;

use super::queries::{components_by_spec, device_counts};
use crate::common::MAX_LIST_ROWS;

#[derive(Inject)]
pub struct ListSpecsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListSpecsRequest, Vec<SpecModel>> for ListSpecsHandler {
    async fn handle(&mut self, _: ListSpecsRequest) -> ActionResult<Vec<SpecModel>> {
        let products = linq!(self.ctx.set::<Product>();)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;
        let mut specs = linq!(self.ctx.set::<Spec>();)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;
        specs.sort_by_key(|s| (s.sort_order, s.id.clone()));

        let ids: Vec<String> = specs.iter().map(|s| s.id.clone()).collect();
        let components = components_by_spec(&mut self.ctx, &ids).await?;
        let counts = device_counts(&mut self.ctx, &ids).await?;

        Ok(specs
            .into_iter()
            .map(|s| {
                let name = products
                    .iter()
                    .find(|p| p.id == s.product_id)
                    .map(|p| p.name.as_str())
                    .unwrap_or("");
                let comps = components.get(&s.id).cloned().unwrap_or_default();
                let count = counts.get(&s.id).copied().unwrap_or(0);
                spec_model(&s, name, comps, count)
            })
            .collect())
    }
}
