//! Category contracts — hierarchical categories with tree response.

use serde::{Deserialize, Serialize};
use webx::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryModel {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub parent_id: Option<String>,
    pub sort_order: i32,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryTreeNode {
    #[serde(flatten)]
    pub category: CategoryModel,
    pub children: Vec<CategoryTreeNode>,
    pub level: u32,
}

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListCategoriesRequest;

#[get("/api/categories")]
impl IRequest<Vec<CategoryTreeNode>> for ListCategoriesRequest {}

#[claims]
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

#[claims]
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

#[claims]
#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct DeleteCategoryRequest {
    pub id: String,
}

#[delete("/api/categories/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteCategoryRequest {}
