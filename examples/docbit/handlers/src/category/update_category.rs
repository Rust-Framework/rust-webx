//! `PUT /api/categories/{id}` — update name / sort order (admin).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::category::{CategoryModel, UpdateCategoryRequest};
use docbit_domain::entities::Category;

use crate::common::{parse_id, save_changes};

#[derive(Inject)]
pub struct UpdateCategoryHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateCategoryRequest, CategoryModel> for UpdateCategoryHandler {
    async fn handle(&mut self, req: UpdateCategoryRequest) -> ActionResult<CategoryModel> {
        let id = parse_id(&req.id)?;
        let q = id.clone();

        let cat = linq!(self.ctx.set::<Category>(), |c: Category| c.id == q)
            .first_or_default()
            .await?
            .ok_or_else(|| Error::not_found("Category not found"))?;

        let updated = {
            let mut updated = None;
            self.ctx.edit(cat, |c| {
                c.patch(req);
                updated = Some(c.clone());
            });
            updated.ok_or_else(|| Error::internal("Category patch did not run"))?
        };
        self.ctx.detect_changes();

        save_changes(&mut self.ctx).await?;

        Ok(CategoryModel::from(&updated))
    }
}
