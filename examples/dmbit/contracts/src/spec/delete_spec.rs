//! `DELETE /api/specs/{id}` — soft-delete a spec, retire its devices.

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct DeleteSpecRequest {
    pub id: String,
}

#[delete("/api/specs/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteSpecRequest {}
