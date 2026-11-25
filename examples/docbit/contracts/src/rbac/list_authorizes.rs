//! `GET /api/authorizes` — every role ↔ resource link (admin).

use serde::Deserialize;
use webx::*;

use super::models::AuthorizeModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListAuthorizesRequest;

#[get("/api/authorizes")]
#[authorize(role = "admin")]
impl IRequest<Vec<AuthorizeModel>> for ListAuthorizesRequest {}
