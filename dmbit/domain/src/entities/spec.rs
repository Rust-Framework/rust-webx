//! Spec entity — 设备规格定义.

use rust_ef::prelude::*;

use super::product::Product;

#[derive(Debug, Clone, EntityType)]
#[table("specs")]
pub struct Spec {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[foreign_key(Product)]
    #[index]
    #[max_length(36)]
    pub product_id: String,
    /// 规格编码（业务唯一键），如 CMP-BASE / CMP-5090 / STO-8TB
    #[required]
    #[max_length(50)]
    #[unique]
    pub code: String,
    /// 品牌短码
    #[required]
    #[max_length(100)]
    pub brand: String,
    /// 机箱参数明细（多行「键：值」）
    #[required]
    pub parameters: String,
    #[required]
    #[max_length(20)]
    pub unit: String,
    /// 计划数量（此规格下 Device 的预期/计划数量）
    #[required]
    pub planned_quantity: i32,
    #[required]
    pub sort_order: i32,
    #[index]
    #[max_length(36)]
    pub created_id: Option<String>,
    #[required]
    pub created_at: i64,
    #[index]
    #[max_length(36)]
    pub updated_id: Option<String>,
    #[required]
    pub updated_at: i64,
    #[required]
    #[index]
    pub is_deleted: bool,
    #[navigation]
    pub product: BelongsTo<Product>,
    #[navigation]
    pub components: HasMany<super::spec_component::SpecComponent>,
    #[navigation]
    pub devices: HasMany<super::device::Device>,
}
