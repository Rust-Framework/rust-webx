//! Goods entity — 台账行（从表）.

use rust_ef::prelude::*;

use super::product::Product;

#[derive(Debug, Clone, EntityType)]
#[table("goods")]
pub struct Goods {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[foreign_key(Product)]
    #[index]
    #[max_length(36)]
    pub product_id: String,
    /// 品牌短码 / 型号系列短名（非整机配置摘要）
    #[required]
    #[max_length(100)]
    pub brand: String,
    /// 机箱参数明细（多行「键：值」）
    #[required]
    pub parameters: String,
    #[required]
    #[max_length(20)]
    pub unit: String,
    #[required]
    pub quantity: i32,
    /// 运行中 / 联调中 / 待上架 / 已交付
    #[required]
    #[max_length(20)]
    pub status: String,
    /// 机房机位，如 A区·R12
    #[max_length(100)]
    pub location: String,
    /// 资产编码
    #[max_length(50)]
    pub asset_code: String,
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
}
