//! Goods contracts — DTO types (ComponentModel, GoodsModel).
//!
//! NOTE: Goods routes have been replaced by Spec routes (/api/specs).
//! GoodsModel is retained for backward compatibility with admin panel.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ComponentModel {
    #[serde(default)]
    pub id: String,
    /// accelerator | disk
    pub kind: String,
    pub model: String,
    /// Disk capacity in GB (decimal). Must be `0` for accelerator.
    #[serde(default)]
    pub capacity_gb: i64,
    pub qty_per_unit: i32,
    #[serde(default)]
    pub sort_order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoodsModel {
    pub id: String,
    pub product_id: String,
    pub product_name: String,
    pub brand: String,
    pub parameters: String,
    pub unit: String,
    pub quantity: i32,
    /// 运行中 / 联调中 / 待上架 / 已交付
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub asset_code: String,
    pub sort_order: i32,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub components: Vec<ComponentModel>,
    /// 规格编码（Spec.code 映射，向后兼容）
    #[serde(default)]
    pub code: String,
    /// 已有设备数量
    #[serde(default)]
    pub device_count: i32,
    /// 计划数量（Spec 模型使用）
    #[serde(default)]
    pub planned_quantity: i32,
}
