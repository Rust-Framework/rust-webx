//! SpecComponent — 规格部件（加速卡 / 硬盘 / NPU 等）.

use rust_ef::prelude::*;

use super::spec::Spec;

#[derive(Debug, Clone, EntityType)]
#[table("spec_components")]
pub struct SpecComponent {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[foreign_key(Spec)]
    #[index]
    #[max_length(36)]
    pub spec_id: String,
    /// accelerator | disk | npu | gpu | ...
    #[required]
    #[max_length(20)]
    pub kind: String,
    #[required]
    #[max_length(80)]
    pub model: String,
    /// 硬盘容量（GB 整数）。加速卡/NPU 为 0。
    #[required]
    pub capacity_gb: i64,
    /// 单台设备安装数量
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
    pub spec: BelongsTo<Spec>,
}
