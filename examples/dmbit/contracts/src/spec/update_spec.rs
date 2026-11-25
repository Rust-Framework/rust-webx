//! `PUT /api/specs/{id}` — partial update of a spec (optionally replacing its
//! whole component list).

use serde::Deserialize;
use webx::*;

use super::models::SpecModel;
use crate::goods::models::ComponentModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct UpdateSpecRequest {
    #[frompath]
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
