//! `POST /api/specs` — create a spec with its component list.

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::spec::{CreateSpecRequest, SpecModel};
use dmbit_domain::entities::Spec;

use super::queries::{
    assert_code_available, replace_components, require_product_name, spec_model_for,
};
use crate::common::save_changes;

#[derive(Inject)]
pub struct CreateSpecHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateSpecRequest, SpecModel> for CreateSpecHandler {
    async fn handle(&mut self, req: CreateSpecRequest) -> ActionResult<SpecModel> {
        // 1. Validate and resolve every trusted input first.
        let components = req.components.clone();
        let entity = Spec::try_from(req)?;
        let product_name = require_product_name(&mut self.ctx, &entity.product_id).await?;
        assert_code_available(&mut self.ctx, &entity.code, None).await?;

        // 2. Add the spec and its components, then commit once.
        let spec_id = entity.id.clone();
        self.ctx.add(entity.clone());
        replace_components(&mut self.ctx, &spec_id, &components).await?;
        save_changes(&mut self.ctx).await?;

        spec_model_for(&mut self.ctx, &entity, &product_name).await
    }
}
