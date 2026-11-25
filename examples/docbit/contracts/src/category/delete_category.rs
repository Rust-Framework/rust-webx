//! `DELETE /api/categories/{id}` — admin only, soft delete.

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct DeleteCategoryRequest {
    pub id: String,
}

#[delete("/api/categories/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteCategoryRequest {}
