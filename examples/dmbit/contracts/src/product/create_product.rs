//! `POST /api/products` — create a product (机型).

use serde::Deserialize;
use webx::*;

use super::models::ProductModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
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
