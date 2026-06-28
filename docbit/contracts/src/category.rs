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

#[derive(Default)]
pub struct ListCategoriesRequest;

#[get("/api/categories")]
impl IRequest<Vec<CategoryTreeNode>> for ListCategoriesRequest {}

#[derive(Default, Deserialize)]
pub struct CreateCategoryRequest {
    #[serde(skip)]
    pub claims: Option<Box<dyn IClaims>>,
    pub name: String,
    pub slug: String,
    pub parent_id: Option<i32>,
    pub sort_order: i32,
}
impl_claims_carrier!(CreateCategoryRequest);

#[post("/api/categories")]
#[authorize(role = "admin")]
impl IRequest<CategoryModel> for CreateCategoryRequest {}

#[derive(Default, Deserialize)]
pub struct UpdateCategoryRequest {
    #[serde(skip)]
    pub claims: Option<Box<dyn IClaims>>,
    pub id: String,
    pub name: Option<String>,
    pub sort_order: Option<i32>,
}
impl_claims_carrier!(UpdateCategoryRequest);

#[put("/api/categories/{id}")]
#[authorize(role = "admin")]
impl IRequest<CategoryModel> for UpdateCategoryRequest {}

#[derive(Default)]
pub struct DeleteCategoryRequest {
    pub claims: Option<Box<dyn IClaims>>,
    pub id: String,
}
impl_claims_carrier!(DeleteCategoryRequest);

#[delete("/api/categories/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteCategoryRequest {}
