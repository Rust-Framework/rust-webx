//! `DELETE /api/categories/{id}` — soft delete (admin).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::category::DeleteCategoryRequest;
use docbit_domain::entities::Category;

use crate::common::{now_secs, operator_id, parse_id, save_changes};

#[derive(Inject)]
pub struct DeleteCategoryHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteCategoryRequest, String> for DeleteCategoryHandler {
    async fn handle(&mut self, req: DeleteCategoryRequest) -> ActionResult<String> {
        let id = parse_id(&req.id)?;
        let q = id.clone();

        let cat = linq!(self.ctx.set::<Category>(), |c: Category| c.id == q)
            .first_or_default()
            .await?
            .ok_or_else(|| Error::not_found("Category not found"))?;

        self.ctx.edit(cat, |c| {
            c.is_deleted = true;
            c.updated_id = operator_id();
            c.updated_at = now_secs();
        });
        self.ctx.detect_changes();

        save_changes(&mut self.ctx).await?;

        tracing::info!("[Category] Soft-deleted: {}", id);
        Ok(format!("Deleted category {}", id))
    }
}
