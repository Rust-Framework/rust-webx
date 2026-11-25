//! `POST /api/specs/{id}/devices/generate` — batch-generate devices for a spec.

use serde::{Deserialize, Serialize};
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GenerateDevicesRequest {
    #[frompath]
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
