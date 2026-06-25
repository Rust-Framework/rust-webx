use crate::services::site::SiteConfig;
use rust_webapp::*;

pub struct SiteInfoRequest;

#[get("/api/site")]
impl IRequest<SiteConfig> for SiteInfoRequest {}
