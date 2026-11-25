//! `GET /api/devices/{id}` — one device.

use rust_ef::db_context::DbContext;
use webx::*;

use dmbit_contracts::device::{DeviceModel, GetDeviceRequest};
use dmbit_domain::mappers::device_mapper::device_model;

use super::queries::require_device;
use crate::common::parse_id;
use crate::spec::queries::spec_with_product;

#[derive(Inject)]
pub struct GetDeviceHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetDeviceRequest, DeviceModel> for GetDeviceHandler {
    async fn handle(&mut self, req: GetDeviceRequest) -> ActionResult<DeviceModel> {
        let id = parse_id(&req.id)?;
        let device = require_device(&mut self.ctx, &id).await?;
        let (spec, product_name) = spec_with_product(&mut self.ctx, &device.spec_id).await?;
        Ok(device_model(&device, &spec.code, &product_name))
    }
}
