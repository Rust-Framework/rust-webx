//! `POST /api/specs` — create a spec with its component list.

use serde::Deserialize;
use webx::*;

use super::models::SpecModel;
use crate::goods::models::ComponentModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
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
