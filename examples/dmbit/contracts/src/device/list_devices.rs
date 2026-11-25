//! `GET /api/devices` — the flat device list, offset-paginated.
//!
//! Device instances grow with the fleet, so this is the one list endpoint that
//! is paginated: `paging` is a nested request model (`page`/`page_size`, absent →
//! `1`/`20`), and the response is the framework `PagedResult` envelope.

use serde::Deserialize;
use webx::*;

use super::models::DeviceModel;

#[derive(Deserialize, WebxRequestMeta)]
pub struct ListDevicesRequest {
    #[fromquery(nested)]
    pub paging: PagedRequest,
}

#[get("/api/devices")]
#[authorize(role = "admin")]
impl IRequest<PagedResult<DeviceModel>> for ListDevicesRequest {}
