//! `GET /api/products/{id}` — one product with its spec tree.

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::product::{GetProductRequest, ProductModel};

use super::queries::{product_model_for, require_product};
use crate::common::parse_id;

#[derive(Inject)]
pub struct GetProductHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetProductRequest, ProductModel> for GetProductHandler {
    async fn handle(&mut self, req: GetProductRequest) -> ActionResult<ProductModel> {
        let id = parse_id(&req.id)?;
        let product = require_product(&mut self.ctx, &id).await?;
        product_model_for(&mut self.ctx, &product).await
    }
}
