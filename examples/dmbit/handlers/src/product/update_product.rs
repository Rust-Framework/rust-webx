//! `PUT /api/products/{id}` — partial update of a product.

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::product::{ProductModel, UpdateProductRequest};
use dmbit_domain::mappers::product_mapper::normalize_update_product;

use super::queries::{assert_code_available, product_model_for, require_product};
use crate::common::{now_secs, operator_id, parse_id, save_changes};

#[derive(Inject)]
pub struct UpdateProductHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateProductRequest, ProductModel> for UpdateProductHandler {
    async fn handle(&mut self, req: UpdateProductRequest) -> ActionResult<ProductModel> {
        let id = parse_id(&req.id)?;
        let mut req = req;
        normalize_update_product(&mut req)?;

        // Code uniqueness needs the context, so it resolves before the row is
        // loaded; the rest of the fields were validated by the normalizer.
        if let Some(code) = req.code.as_deref() {
            assert_code_available(&mut self.ctx, code, Some(&id)).await?;
        }

        let row = require_product(&mut self.ctx, &id).await?;
        let now = now_secs();
        let actor = operator_id();
        let mut updated = None;
        self.ctx.edit(row, |product| {
            product.patch(req);
            product.updated_at = now;
            product.updated_id = actor.clone();
            updated = Some(product.clone());
        });
        self.ctx.detect_changes();
        save_changes(&mut self.ctx).await?;

        let updated = updated.ok_or_else(|| Error::internal("product patch did not run"))?;
        product_model_for(&mut self.ctx, &updated).await
    }
}
