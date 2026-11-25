//! `GET /api/specs` — every spec.

use serde::Deserialize;
use webx::*;

use super::models::SpecModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListSpecsRequest;

#[get("/api/specs")]
#[authorize(role = "admin")]
impl IRequest<Vec<SpecModel>> for ListSpecsRequest {}
