//! `GET /api/specs/{id}/devices` — the devices of one spec.

use serde::Deserialize;
use webx::*;

use super::models::DeviceModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListSpecDevicesRequest {
    pub id: String,
}

#[get("/api/specs/{id}/devices")]
#[authorize(role = "admin")]
impl IRequest<Vec<DeviceModel>> for ListSpecDevicesRequest {}
