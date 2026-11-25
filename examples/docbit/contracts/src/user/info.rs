//! `GET /api/info` — API server information.

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct InfoRequest;

/// Get API server information including version and endpoint count.
#[get("/api/info")]
impl IRequest<String> for InfoRequest {}
