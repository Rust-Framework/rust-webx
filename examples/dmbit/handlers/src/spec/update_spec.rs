//! `PUT /api/specs/{id}` — partial update of a spec (optionally replacing its
//! whole component list).

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::spec::{SpecModel, UpdateSpecRequest};
use dmbit_domain::mappers::spec_mapper::normalize_update_spec;

use super::queries::{
    assert_code_available, replace_components, require_product_name, require_spec, spec_model_for,
};
use crate::common::{now_secs, operator_id, parse_id, save_changes};

#[derive(Inject)]
pub struct UpdateSpecHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateSpecRequest, SpecModel> for UpdateSpecHandler {
    async fn handle(&mut self, req: UpdateSpecRequest) -> ActionResult<SpecModel> {
        let id = parse_id(&req.id)?;
        let mut req = req;
        normalize_update_spec(&mut req)?;

        // Uniqueness and the target product need the context, so they resolve
        // before the row is loaded for change detection.
        if let Some(code) = req.code.as_deref() {
            assert_code_available(&mut self.ctx, code, Some(&id)).await?;
        }
        let new_product = match req.product_id.as_deref() {
            Some(raw) => {
                let product_id = parse_id(raw)?;
                let name = require_product_name(&mut self.ctx, &product_id).await?;
                Some((product_id, name))
            }
            None => None,
        };
        let components = req.components.clone();

        let row = require_spec(&mut self.ctx, &id).await?;
        let now = now_secs();
        let actor = operator_id();
        let mut updated = None;
        self.ctx.edit(row, |spec| {
            spec.patch(req);
            if let Some((product_id, _)) = &new_product {
                spec.product_id = product_id.clone();
            }
            spec.updated_at = now;
            spec.updated_id = actor.clone();
            updated = Some(spec.clone());
        });
        self.ctx.detect_changes();

        if let Some(components) = components.as_ref() {
            replace_components(&mut self.ctx, &id, components).await?;
        }
        save_changes(&mut self.ctx).await?;

        let updated = updated.ok_or_else(|| Error::internal("spec patch did not run"))?;
        let product_name = match new_product {
            Some((_, name)) => name,
            None => require_product_name(&mut self.ctx, &updated.product_id).await?,
        };
        spec_model_for(&mut self.ctx, &updated, &product_name).await
    }
}
