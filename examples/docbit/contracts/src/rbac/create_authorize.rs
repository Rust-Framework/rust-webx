//! `POST /api/authorizes` — link a resource to a role (admin, idempotent).

use serde::Deserialize;
use webx::*;

use super::models::AuthorizeModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct CreateAuthorizeRequest {
    pub role_id: String,
    pub resource_id: String,
}

#[post("/api/authorizes")]
#[authorize(role = "admin")]
impl IRequest<AuthorizeModel> for CreateAuthorizeRequest {}
