//! `GET /api/docs` — the served documentation works.

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListDocWorksRequest;

#[get("/api/docs")]
impl IRequest<Vec<String>> for ListDocWorksRequest {}
