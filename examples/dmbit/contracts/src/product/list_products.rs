//! `GET /api/products` — the product tree the admin panel renders.

use serde::Deserialize;
use webx::*;

use super::models::ProductModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListProductsRequest;

#[get("/api/products")]
#[authorize(role = "admin")]
impl IRequest<Vec<ProductModel>> for ListProductsRequest {}
