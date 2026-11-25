//! `POST /api/categories` — create a category (admin).

use rust_ef::db_context::DbContext;
use webx::*;

use docbit_contracts::category::{CategoryModel, CreateCategoryRequest};
use docbit_domain::entities::Category;

use crate::common::save_changes;

#[derive(Inject)]
pub struct CreateCategoryHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateCategoryRequest, CategoryModel> for CreateCategoryHandler {
    async fn handle(&mut self, req: CreateCategoryRequest) -> ActionResult<CategoryModel> {
        let entity = Category::try_from(req)?;
        let (id, name) = (entity.id.clone(), entity.name.clone());
        let model = CategoryModel::from(&entity);

        self.ctx.add(entity);

        save_changes(&mut self.ctx).await?;

        tracing::info!("[Category] Created: {} ({})", name, id);
        Ok(model)
    }
}
