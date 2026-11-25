//! `POST /api/devices` — create one device instance.

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::device::{CreateDeviceRequest, DeviceModel};
use dmbit_domain::entities::Device;
use dmbit_domain::mappers::device_mapper::device_model;

use super::queries::assert_asset_code_available;
use crate::common::save_changes;
use crate::spec::queries::spec_with_product;

#[derive(Inject)]
pub struct CreateDeviceHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateDeviceRequest, DeviceModel> for CreateDeviceHandler {
    async fn handle(&mut self, req: CreateDeviceRequest) -> ActionResult<DeviceModel> {
        let entity = Device::try_from(req)?;
        let (spec, product_name) = spec_with_product(&mut self.ctx, &entity.spec_id).await?;
        assert_asset_code_available(&mut self.ctx, &entity.asset_code, None).await?;

        let model = device_model(&entity, &spec.code, &product_name);

        self.ctx.add(entity);
        save_changes(&mut self.ctx).await?;

        Ok(model)
    }
}
