//! `DELETE /api/devices/{id}` — soft-delete one device.

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct DeleteDeviceRequest {
    pub id: String,
}

#[delete("/api/devices/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteDeviceRequest {}
