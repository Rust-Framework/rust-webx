//! GoodsComponent — countable parts (accelerator cards / disks) per goods row.

use rust_ef::prelude::*;

use super::goods::Goods;

#[derive(Debug, Clone, EntityType)]
#[table("goods_components")]
pub struct GoodsComponent {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[foreign_key(Goods)]
    #[index]
    #[max_length(36)]
    pub goods_id: String,
    /// accelerator | disk
    #[required]
    #[max_length(20)]
    pub kind: String,
    #[required]
    #[max_length(80)]
    pub model: String,
    /// Disk capacity in **GB** (decimal). `0` for accelerator. Source of truth for capacity math.
    #[required]
    pub capacity_gb: i64,
    /// count installed on each server unit
    #[required]
    pub qty_per_unit: i32,
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
    pub goods: BelongsTo<Goods>,
}
