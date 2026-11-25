//! Spec read model.

use serde::{Deserialize, Serialize};

use crate::goods::models::ComponentModel;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecModel {
    pub id: String,
    pub product_id: String,
    pub product_name: String,
    /// 规格编码（业务唯一键）
    pub code: String,
    pub brand: String,
    pub parameters: String,
    pub unit: String,
    pub planned_quantity: i32,
    pub sort_order: i32,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<ComponentModel>,
    /// 当前实际设备数量（Device 计数）
    #[serde(default)]
    pub device_count: i32,
}
