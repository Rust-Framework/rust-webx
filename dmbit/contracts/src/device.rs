//! Device contracts — 设备实例 CRUD + 批量生成.

use rust_webx::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceModel {
    pub id: String,
    pub spec_id: String,
    pub spec_code: String,
    pub product_name: String,
    /// 运行中 / 联调中 / 待上架 / 已交付 / 已淘汰
    pub status: String,
    pub location: String,
    pub asset_code: String,
    pub serial_no: String,
    pub sort_order: i32,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeviceFilter {
    #[serde(default)]
    pub spec_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub search: Option<String>,
}

#[claims]
#[derive(Default)]
pub struct ListDevicesRequest;

#[get("/api/devices")]
#[authorize(role = "admin")]
impl IRequest<Vec<DeviceModel>> for ListDevicesRequest {}

#[claims]
#[derive(Default)]
pub struct ListSpecDevicesRequest {
    pub id: String,
}

#[get("/api/specs/{id}/devices")]
#[authorize(role = "admin")]
impl IRequest<Vec<DeviceModel>> for ListSpecDevicesRequest {}

#[claims]
#[derive(Default)]
pub struct GetDeviceRequest {
    pub id: String,
}

#[get("/api/devices/{id}")]
#[authorize(role = "admin")]
impl IRequest<DeviceModel> for GetDeviceRequest {}

#[claims]
#[derive(Default, Deserialize)]
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

#[claims]
#[derive(Default, Deserialize)]
pub struct GenerateDevicesRequest {
    pub id: String,
    #[serde(default)]
    pub spec_id: String,
    /// 要生成的设备数量（默认使用 spec.planned_quantity）
    pub count: Option<i32>,
    /// 资产编码前缀（如 "CMP-BASE-"，自动编号）
    #[serde(default)]
    pub asset_prefix: String,
    /// 起始编号（默认 1）
    #[serde(default = "one")]
    pub start_index: i32,
}

fn one() -> i32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateDevicesResult {
    pub created: i32,
    pub message: String,
}

#[post("/api/specs/{id}/devices/generate")]
#[authorize(role = "admin")]
impl IRequest<GenerateDevicesResult> for GenerateDevicesRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct UpdateDeviceRequest {
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

#[claims]
#[derive(Default)]
pub struct DeleteDeviceRequest {
    pub id: String,
}

#[delete("/api/devices/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteDeviceRequest {}
