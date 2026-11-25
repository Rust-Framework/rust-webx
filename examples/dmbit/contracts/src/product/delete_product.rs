//! `DELETE /api/products/{id}` — soft-delete a product and cascade its specs.

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct DeleteProductRequest {
    pub id: String,
}

#[delete("/api/products/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteProductRequest {}
