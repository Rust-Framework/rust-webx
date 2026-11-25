//! `PUT /api/devices/{id}` — partial update of one device.

use serde::Deserialize;
use webx::*;

use super::models::DeviceModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct UpdateDeviceRequest {
    #[frompath]
    #[serde(default)]
    pub id: String,
    pub spec_id: Option<String>,
    pub status: Option<String>,
    pub location: Option<String>,
    pub asset_code: Option<String>,
    pub serial_no: Option<String>,
    pub sort_order: Option<i32>,
}

#[put("/api/devices/{id}")]
#[authorize(role = "admin")]
impl IRequest<DeviceModel> for UpdateDeviceRequest {}
