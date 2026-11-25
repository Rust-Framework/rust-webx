//! `PUT /api/categories/{id}` — admin only.

use serde::Deserialize;
use webx::*;

use super::models::CategoryModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct UpdateCategoryRequest {
    #[frompath]
    #[serde(default)]
    pub id: String,
    pub name: Option<String>,
    pub sort_order: Option<i32>,
}

#[put("/api/categories/{id}")]
#[authorize(role = "admin")]
impl IRequest<CategoryModel> for UpdateCategoryRequest {}
