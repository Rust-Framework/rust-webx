//! `DELETE /api/specs/{id}` — soft-delete a spec, retire its devices.

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::spec::DeleteSpecRequest;

use super::queries::{require_spec, retire_spec_children};
use crate::common::{now_secs, operator_id, parse_id, save_changes};

#[derive(Inject)]
pub struct DeleteSpecHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteSpecRequest, String> for DeleteSpecHandler {
    async fn handle(&mut self, req: DeleteSpecRequest) -> ActionResult<String> {
        let id = parse_id(&req.id)?;
        let row = require_spec(&mut self.ctx, &id).await?;

        let now = now_secs();
        let actor = operator_id();
        self.ctx.edit(row, |spec| {
            spec.is_deleted = true;
            spec.updated_at = now;
            spec.updated_id = actor;
        });
        retire_spec_children(&mut self.ctx, &id).await?;
        self.ctx.detect_changes();
        save_changes(&mut self.ctx).await?;

        Ok(format!("已删除规格 {}", id))
    }
}
