//! `GET /api/devices` — the flat device list, offset-paginated.
//!
//! This is the list that grows with the fleet, so it is the one endpoint that
//! pages: `PagedRequest` comes from the query string, and the framework's
//! `to_page` runs the `COUNT(*)` and the page off the same filter chain.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use dmbit_contracts::device::{DeviceModel, ListDevicesRequest};
use dmbit_domain::entities::Device;
use dmbit_domain::mappers::device_mapper::device_model;

use crate::spec::queries::spec_name_map;

#[derive(Inject)]
pub struct ListDevicesHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListDevicesRequest, PagedResult<DeviceModel>> for ListDevicesHandler {
    async fn handle(&mut self, req: ListDevicesRequest) -> ActionResult<PagedResult<DeviceModel>> {
        // Stable order: the last column is the unique key, so rows cannot shift
        // between pages.
        let page = linq!(self.ctx.set::<Device>();)
            .order_by_field(Device::sort_order)
            .then_by_field(Device::asset_code)
            .then_by_field(Device::id)
            .to_page(&req.paging)
            .await?;

        let names = spec_name_map(&mut self.ctx).await?;
        Ok(page.map_items(|d| {
            let (code, product_name) = names.get(&d.spec_id).cloned().unwrap_or_default();
            device_model(&d, &code, &product_name)
        }))
    }
}
