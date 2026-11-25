//! `PUT /api/products/{id}` — partial update of a product.

use serde::Deserialize;
use webx::*;

use super::models::ProductModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct UpdateProductRequest {
    #[frompath]
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
