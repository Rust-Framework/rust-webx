//! `DELETE /api/authorizes/{id}` — remove a role ↔ resource link (admin).

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct DeleteAuthorizeRequest {
    pub id: String,
}

#[delete("/api/authorizes/{id}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteAuthorizeRequest {}
