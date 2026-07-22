//! Product contracts — master table CRUD.

use rust_webx::*;
use serde::{Deserialize, Serialize};

use crate::goods::GoodsModel;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductModel {
    pub id: String,
    pub name: String,
    pub code: String,
    /// compute | storage
    pub category: String,
    pub remark: String,
    pub sort_order: i32,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub goods: Vec<GoodsModel>,
}

#[claims]
#[derive(Default)]
pub struct ListProductsRequest;

#[get("/api/products")]
#[authorize(role = "admin")]
impl IRequest<Vec<ProductModel>> for ListProductsRequest {}

#[claims]
#[derive(Default)]
pub struct GetProductRequest {
    pub id: String,
}

#[get("/api/products/{id}")]
#[authorize(role = "admin")]
impl IRequest<ProductModel> for GetProductRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct CreateProductRequest {
    pub name: String,
    pub code: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default)]
    pub remark: String,
    #[serde(default)]
    pub sort_order: i32,
}

fn default_category() -> String {
    "compute".into()
}

#[post("/api/products")]
#[authorize(role = "admin")]
impl IRequest<ProductModel> for CreateProductRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct UpdateProductRequest {
    #[serde(default)]
    pub id: String,
    pub name: Option<String>,
    pub code: Option<String>,
    pub category: Option<String>,
    pub remark: Option<String>,
    pub sort_order: Option<i32>,
}

#[put("/api/products/{id}")]
#[authorize(role = "admin")]
impl IRequest<ProductModel> for UpdateProductRequest {}

#[claims]
#[derive(Default)]
pub struct DeleteProductRequest {
    pub id: String,
}

#[delete("/api/products/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteProductRequest {}
