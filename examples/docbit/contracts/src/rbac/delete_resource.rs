//! `DELETE /api/resources/{id}` — soft delete a resource (admin).

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct DeleteResourceRequest {
    pub id: String,
}

#[delete("/api/resources/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteResourceRequest {}
