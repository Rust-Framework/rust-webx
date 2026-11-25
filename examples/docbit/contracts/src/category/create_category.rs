//! `POST /api/categories` — admin only.

use serde::Deserialize;
use webx::*;

use super::models::CategoryModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct CreateCategoryRequest {
    pub name: String,
    pub slug: String,
    pub parent_id: Option<String>,
    pub sort_order: i32,
}

#[post("/api/categories")]
#[authorize(role = "admin")]
impl IRequest<CategoryModel> for CreateCategoryRequest {}
