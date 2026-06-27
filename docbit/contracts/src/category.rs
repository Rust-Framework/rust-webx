//! Category contracts — hierarchical categories with tree response.

use rust_webapp::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryModel {
    pub id: i32,
    pub name: String,
    pub slug: String,
    pub parent_id: Option<i32>,
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

pub struct ListCategoriesRequest;

#[get("/api/categories")]
impl IRequest<Vec<CategoryTreeNode>> for ListCategoriesRequest {}

#[derive(Deserialize)]
pub struct CreateCategoryRequest {
    pub name: String,
    pub slug: String,
    pub parent_id: Option<i32>,
    pub sort_order: i32,
}

#[post("/api/categories")]
#[authorize(role = "admin")]
impl IRequest<CategoryModel> for CreateCategoryRequest {}

#[derive(Deserialize)]
pub struct UpdateCategoryRequest {
    pub id: String,
    pub name: Option<String>,
    pub sort_order: Option<i32>,
}

#[put("/api/categories/{id}")]
#[authorize(role = "admin")]
impl IRequest<CategoryModel> for UpdateCategoryRequest {}

pub struct DeleteCategoryRequest {
    pub id: String,
}

#[delete("/api/categories/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteCategoryRequest {}
