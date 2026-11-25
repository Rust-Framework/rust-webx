//! `POST /api/inventory/import` — upload a CSV and upsert products / specs.

use webx::*;

use super::models::ImportInventoryResult;

#[derive(WebxRequestMeta)]
pub struct ImportInventoryRequest {
    /// The uploaded CSV. Streamed and spooled by the framework — never
    /// string-encoded into a JSON field.
    pub file: FormFile,
    /// 为 true 时：对已存在的产品编码/台账走更新覆盖；为 false 时若有冲突则只返回冲突清单不落库。
    pub confirm_update: Option<bool>,
}

#[post("/api/inventory/import")]
#[authorize(role = "admin")]
impl IRequest<ImportInventoryResult> for ImportInventoryRequest {}
