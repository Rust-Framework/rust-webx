//! Device read model.

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
