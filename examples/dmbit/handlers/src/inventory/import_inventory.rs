//! `POST /api/inventory/import` — upload a CSV and upsert products / specs.

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::inventory::{ImportInventoryRequest, ImportInventoryResult};

use super::use_cases::import_inventory;

#[derive(Inject)]
pub struct ImportInventoryHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ImportInventoryRequest, ImportInventoryResult> for ImportInventoryHandler {
    async fn handle(&mut self, req: ImportInventoryRequest) -> ActionResult<ImportInventoryResult> {
        import_inventory(&mut self.ctx, req).await
    }
}
