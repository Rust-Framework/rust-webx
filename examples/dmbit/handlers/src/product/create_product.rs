//! `POST /api/products` — create a product (机型).

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::product::{CreateProductRequest, ProductModel};
use dmbit_domain::entities::Product;

use super::queries::assert_code_available;
use crate::common::save_changes;

#[derive(Inject)]
pub struct CreateProductHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateProductRequest, ProductModel> for CreateProductHandler {
    async fn handle(&mut self, req: CreateProductRequest) -> ActionResult<ProductModel> {
        let entity = Product::try_from(req)?;
        assert_code_available(&mut self.ctx, &entity.code, None).await?;

        let model = ProductModel::from(&entity);
        self.ctx.add(entity);
        save_changes(&mut self.ctx).await?;

        Ok(model)
    }
}
