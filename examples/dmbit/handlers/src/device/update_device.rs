//! `PUT /api/devices/{id}` — partial update of one device.

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::device::{DeviceModel, UpdateDeviceRequest};
use dmbit_domain::mappers::device_mapper::{device_model, normalize_update_device};

use super::queries::{assert_asset_code_available, require_device};
use crate::common::{now_secs, operator_id, parse_id, save_changes};
use crate::spec::queries::spec_with_product;

#[derive(Inject)]
pub struct UpdateDeviceHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateDeviceRequest, DeviceModel> for UpdateDeviceHandler {
    async fn handle(&mut self, req: UpdateDeviceRequest) -> ActionResult<DeviceModel> {
        let id = parse_id(&req.id)?;
        let mut req = req;
        normalize_update_device(&mut req)?;

        // Moving a device to another spec must land on an existing spec, and the
        // asset code stays unique: both need the context, so they resolve first.
        if let Some(raw) = req.spec_id.as_deref() {
            let spec_id = parse_id(raw)?;
            spec_with_product(&mut self.ctx, &spec_id).await?;
        }
        if let Some(asset_code) = req.asset_code.as_deref() {
            assert_asset_code_available(&mut self.ctx, asset_code, Some(&id)).await?;
        }

        let row = require_device(&mut self.ctx, &id).await?;
        let now = now_secs();
        let actor = operator_id();
        let new_spec_id = req.spec_id.clone();
        let mut updated = None;
        self.ctx.edit(row, |device| {
            device.patch(req);
            if let Some(spec_id) = new_spec_id {
                device.spec_id = spec_id;
            }
            device.updated_at = now;
            device.updated_id = actor.clone();
            updated = Some(device.clone());
        });
        self.ctx.detect_changes();
        save_changes(&mut self.ctx).await?;

        let updated = updated.ok_or_else(|| Error::internal("device patch did not run"))?;
        let (spec, product_name) = spec_with_product(&mut self.ctx, &updated.spec_id).await?;
        Ok(device_model(&updated, &spec.code, &product_name))
    }
}
