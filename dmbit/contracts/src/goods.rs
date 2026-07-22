//! Goods contracts — inventory rows + countable components.

use rust_webx::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ComponentModel {
    #[serde(default)]
    pub id: String,
    /// accelerator | disk
    pub kind: String,
    pub model: String,
    #[serde(default)]
    pub capacity: String,
    pub qty_per_unit: i32,
    #[serde(default)]
    pub sort_order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoodsModel {
    pub id: String,
    pub product_id: String,
    pub product_name: String,
    pub brand: String,
    pub parameters: String,
    pub unit: String,
    pub quantity: i32,
    /// 运行中 / 联调中 / 待上架 / 已交付
    pub status: String,
    pub location: String,
    pub asset_code: String,
    pub sort_order: i32,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub components: Vec<ComponentModel>,
}

#[derive(Default)]
pub struct ListGoodsRequest;

#[get("/api/goods")]
impl IRequest<Vec<GoodsModel>> for ListGoodsRequest {}

#[derive(Default)]
pub struct ListProductGoodsRequest {
    pub id: String,
}

#[get("/api/products/{id}/goods")]
impl IRequest<Vec<GoodsModel>> for ListProductGoodsRequest {}

#[derive(Default)]
pub struct GetGoodsRequest {
    pub id: String,
}

#[get("/api/goods/{id}")]
impl IRequest<GoodsModel> for GetGoodsRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct CreateGoodsRequest {
    pub product_id: String,
    pub brand: String,
    pub parameters: String,
    pub unit: String,
    pub quantity: i32,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub asset_code: String,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default)]
    pub components: Vec<ComponentModel>,
}

fn default_status() -> String {
    "待上架".into()
}

#[post("/api/goods")]
#[authorize(role = "admin")]
impl IRequest<GoodsModel> for CreateGoodsRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct UpdateGoodsRequest {
    #[serde(default)]
    pub id: String,
    pub product_id: Option<String>,
    pub brand: Option<String>,
    pub parameters: Option<String>,
    pub unit: Option<String>,
    pub quantity: Option<i32>,
    pub status: Option<String>,
    pub location: Option<String>,
    pub asset_code: Option<String>,
    pub sort_order: Option<i32>,
    /// When present, replace the full component list for this goods row.
    pub components: Option<Vec<ComponentModel>>,
}

#[put("/api/goods/{id}")]
#[authorize(role = "admin")]
impl IRequest<GoodsModel> for UpdateGoodsRequest {}

#[claims]
#[derive(Default)]
pub struct DeleteGoodsRequest {
    pub id: String,
}

#[delete("/api/goods/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteGoodsRequest {}
