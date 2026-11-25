//! `GET /api/products/{id}` — one product with its spec tree.

use serde::Deserialize;
use webx::*;

use super::models::ProductModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetProductRequest {
    pub id: String,
}

#[get("/api/products/{id}")]
#[authorize(role = "admin")]
impl IRequest<ProductModel> for GetProductRequest {}
