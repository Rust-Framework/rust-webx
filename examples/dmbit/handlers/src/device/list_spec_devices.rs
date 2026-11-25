//! `GET /api/specs/{id}/devices` — the devices of one spec.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use dmbit_contracts::device::{DeviceModel, ListSpecDevicesRequest};
use dmbit_domain::entities::Device;
use dmbit_domain::mappers::device_mapper::device_model;

use crate::common::{parse_id, MAX_LIST_ROWS};
use crate::spec::queries::spec_with_product;

#[derive(Inject)]
pub struct ListSpecDevicesHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListSpecDevicesRequest, Vec<DeviceModel>> for ListSpecDevicesHandler {
    async fn handle(&mut self, req: ListSpecDevicesRequest) -> ActionResult<Vec<DeviceModel>> {
        let spec_id = parse_id(&req.id)?;
        let (spec, product_name) = spec_with_product(&mut self.ctx, &spec_id).await?;

        let q = spec_id.to_string();
        let mut devices = linq!(self.ctx.set::<Device>(), |d: Device| d.spec_id == q)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;
        devices.sort_by(|a, b| {
            a.sort_order
                .cmp(&b.sort_order)
                .then(a.asset_code.cmp(&b.asset_code))
        });

        Ok(devices
            .into_iter()
            .map(|d| device_model(&d, &spec.code, &product_name))
            .collect())
    }
}
