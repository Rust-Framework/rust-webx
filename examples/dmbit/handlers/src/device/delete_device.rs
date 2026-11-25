//! `DELETE /api/devices/{id}` — soft-delete one device.

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::device::DeleteDeviceRequest;

use super::queries::require_device;
use crate::common::{now_secs, operator_id, parse_id, save_changes};

#[derive(Inject)]
pub struct DeleteDeviceHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteDeviceRequest, String> for DeleteDeviceHandler {
    async fn handle(&mut self, req: DeleteDeviceRequest) -> ActionResult<String> {
        let id = parse_id(&req.id)?;
        let row = require_device(&mut self.ctx, &id).await?;

        let now = now_secs();
        let actor = operator_id();
        self.ctx.edit(row, |device| {
            device.is_deleted = true;
            device.updated_at = now;
            device.updated_id = actor;
        });
        self.ctx.detect_changes();
        save_changes(&mut self.ctx).await?;

        Ok(format!("已删除设备 {}", id))
    }
}
