//! Inventory CSV import / export.

use rust_webx::*;
use serde::{Deserialize, Serialize};

/// Filename offered to the browser when exporting the inventory.
pub const INVENTORY_EXPORT_FILE_NAME: &str = "智算机房规格清单.csv";

#[claims]
#[derive(Default, Deserialize)]
pub struct ExportInventoryRequest;

#[get("/api/inventory/export")]
#[authorize(role = "admin")]
impl IRequest<ResponseData> for ExportInventoryRequest {}

#[claims]
#[derive(Deserialize)]
pub struct ImportInventoryRequest {
    /// The uploaded CSV. Streamed and spooled by the framework — never
    /// string-encoded into a JSON field.
    pub file: FormFile,
    /// 为 true 时：对已存在的产品编码/台账走更新覆盖；为 false 时若有冲突则只返回冲突清单不落库。
    #[serde(default)]
    pub confirm_update: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImportInventoryResult {
    pub products_upserted: i32,
    pub goods_upserted: i32,
    pub components_written: i32,
    pub message: String,
    /// 存在编号冲突且未确认更新时为 true（此时未写入数据库）
    #[serde(default)]
    pub needs_confirm: bool,
    /// 与库中产品编码冲突的编码列表
    #[serde(default)]
    pub conflict_product_codes: Vec<String>,
    /// 将更新的已有台账摘要（品牌 / 资产编码 / 机位）
    #[serde(default)]
    pub conflict_goods_labels: Vec<String>,
}

#[post("/api/inventory/import")]
#[authorize(role = "admin")]
impl IRequest<ImportInventoryResult> for ImportInventoryRequest {}
