//! `GET /api/products` — the product tree the admin panel renders.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use dmbit_contracts::product::{ListProductsRequest, ProductModel};
use dmbit_domain::entities::{Product, Spec};
use dmbit_domain::mappers::product_mapper::product_with_specs;

use crate::common::MAX_LIST_ROWS;
use crate::spec::queries::spec_models_for;

#[derive(Inject)]
pub struct ListProductsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListProductsRequest, Vec<ProductModel>> for ListProductsHandler {
    async fn handle(&mut self, _: ListProductsRequest) -> ActionResult<Vec<ProductModel>> {
        let mut products = linq!(self.ctx.set::<Product>();)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;
        products.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.name.cmp(&b.name)));

        let specs = linq!(self.ctx.set::<Spec>();)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;

        let mut result = Vec::with_capacity(products.len());
        for product in &products {
            let own: Vec<&Spec> = specs
                .iter()
                .filter(|s| s.product_id == product.id)
                .collect();
            let models = spec_models_for(&mut self.ctx, &own, &product.name).await?;
            result.push(product_with_specs(product, &models));
        }
        Ok(result)
    }
}
