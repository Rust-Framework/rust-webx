//! `GET /api/site` — the public site configuration.

use serde::Deserialize;
use webx::*;

use super::models::SiteConfig;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct SiteInfoRequest;

#[get("/api/site")]
impl IRequest<SiteConfig> for SiteInfoRequest {}
