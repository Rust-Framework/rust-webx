//! `GET /api/products/{id}/specs` — the specs of one product.

use serde::Deserialize;
use webx::*;

use super::models::SpecModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListProductSpecsRequest {
    pub id: String,
}

#[get("/api/products/{id}/specs")]
#[authorize(role = "admin")]
impl IRequest<Vec<SpecModel>> for ListProductSpecsRequest {}
