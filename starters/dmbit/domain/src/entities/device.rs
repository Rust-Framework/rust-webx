//! Device entity — 设备实例（每台具体设备）.

use rust_ef::prelude::*;

use super::spec::Spec;

#[derive(Debug, Clone, EntityType)]
#[table("devices")]
pub struct Device {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[foreign_key(Spec)]
    #[index]
    #[max_length(36)]
    pub spec_id: String,
    /// 运行中 / 联调中 / 待上架 / 已交付 / 已淘汰
    #[required]
    #[max_length(20)]
    pub status: String,
    /// 机房机位，如 A区·R12
    #[max_length(100)]
    pub location: String,
    /// 资产编码（全局唯一）
    #[required]
    #[max_length(50)]
    #[unique]
    pub asset_code: String,
    /// 序列号（可选）
    #[max_length(100)]
    pub serial_no: String,
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
