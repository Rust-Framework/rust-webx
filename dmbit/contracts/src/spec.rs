//! Spec contracts — 设备规格 CRUD.

use rust_webx::*;
use serde::{Deserialize, Serialize};

use crate::goods::ComponentModel;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecModel {
    pub id: String,
    pub product_id: String,
    pub product_name: String,
    /// 规格编码（业务唯一键）
    pub code: String,
    pub brand: String,
    pub parameters: String,
    pub unit: String,
    pub planned_quantity: i32,
    pub sort_order: i32,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<ComponentModel>,
    /// 当前实际设备数量（Device 计数）
    #[serde(default)]
    pub device_count: i32,
}

#[claims]
#[derive(Default)]
pub struct ListSpecsRequest;

#[get("/api/specs")]
#[authorize(role = "admin")]
impl IRequest<Vec<SpecModel>> for ListSpecsRequest {}

#[claims]
#[derive(Default)]
pub struct ListProductSpecsRequest {
    pub id: String,
}

#[get("/api/products/{id}/specs")]
#[authorize(role = "admin")]
impl IRequest<Vec<SpecModel>> for ListProductSpecsRequest {}

#[claims]
#[derive(Default)]
pub struct GetSpecRequest {
    pub id: String,
}

#[get("/api/specs/{id}")]
#[authorize(role = "admin")]
impl IRequest<SpecModel> for GetSpecRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct CreateSpecRequest {
    pub product_id: String,
    pub code: String,
    pub brand: String,
    pub parameters: String,
    #[serde(default = "default_unit")]
    pub unit: String,
    #[serde(default)]
    pub planned_quantity: i32,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default)]
    pub components: Vec<ComponentModel>,
}

fn default_unit() -> String {
    "台".into()
}

#[post("/api/specs")]
#[authorize(role = "admin")]
impl IRequest<SpecModel> for CreateSpecRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct UpdateSpecRequest {
    #[serde(default)]
    pub id: String,
    pub product_id: Option<String>,
    pub code: Option<String>,
    pub brand: Option<String>,
    pub parameters: Option<String>,
    pub unit: Option<String>,
    pub planned_quantity: Option<i32>,
    pub sort_order: Option<i32>,
    /// When present, replace the full component list for this spec.
    pub components: Option<Vec<ComponentModel>>,
}

#[put("/api/specs/{id}")]
#[authorize(role = "admin")]
impl IRequest<SpecModel> for UpdateSpecRequest {}

#[claims]
#[derive(Default)]
pub struct DeleteSpecRequest {
    pub id: String,
}

#[delete("/api/specs/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteSpecRequest {}
