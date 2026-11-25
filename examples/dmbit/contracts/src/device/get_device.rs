//! `GET /api/devices/{id}` — one device.

use serde::Deserialize;
use webx::*;

use super::models::DeviceModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetDeviceRequest {
    pub id: String,
}

#[get("/api/devices/{id}")]
#[authorize(role = "admin")]
impl IRequest<DeviceModel> for GetDeviceRequest {}
