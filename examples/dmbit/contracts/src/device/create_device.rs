//! `POST /api/devices` — create one device instance.

use serde::Deserialize;
use webx::*;

use super::models::DeviceModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct CreateDeviceRequest {
    pub spec_id: String,
    #[serde(default = "default_device_status")]
    pub status: String,
    #[serde(default)]
    pub location: String,
    pub asset_code: String,
    #[serde(default)]
    pub serial_no: String,
    #[serde(default)]
    pub sort_order: i32,
}

fn default_device_status() -> String {
    "待上架".into()
}

#[post("/api/devices")]
#[authorize(role = "admin")]
impl IRequest<DeviceModel> for CreateDeviceRequest {}
