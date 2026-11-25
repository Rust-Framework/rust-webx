//! `GET /api/specs/{id}` — one spec with its components.

use serde::Deserialize;
use webx::*;

use super::models::SpecModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetSpecRequest {
    pub id: String,
}

#[get("/api/specs/{id}")]
#[authorize(role = "admin")]
impl IRequest<SpecModel> for GetSpecRequest {}
