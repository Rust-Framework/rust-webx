//! `DELETE /api/products/{id}` — soft-delete a product and cascade its specs.

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::product::DeleteProductRequest;

use super::queries::{require_product, specs_of_product};
use crate::common::{now_secs, operator_id, parse_id, save_changes};
use crate::spec::queries::retire_spec_children;

#[derive(Inject)]
pub struct DeleteProductHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteProductRequest, String> for DeleteProductHandler {
    async fn handle(&mut self, req: DeleteProductRequest) -> ActionResult<String> {
        let id = parse_id(&req.id)?;
        let row = require_product(&mut self.ctx, &id).await?;

        let now = now_secs();
        let actor = operator_id();
        self.ctx.edit(row, |product| {
            product.is_deleted = true;
            product.updated_at = now;
            product.updated_id = actor;
        });

        // Cascade: specs → components soft-deleted, devices retired.
        let specs = specs_of_product(&mut self.ctx, &id).await?;
        for spec in specs {
            let spec_id = spec.id.clone();
            self.ctx.edit(spec, |tracked| {
                tracked.is_deleted = true;
                tracked.updated_at = now;
                tracked.updated_id = operator_id();
            });
            retire_spec_children(&mut self.ctx, &spec_id).await?;
        }

        self.ctx.detect_changes();
        save_changes(&mut self.ctx).await?;

        Ok(format!("已删除产品 {}", id))
    }
}
