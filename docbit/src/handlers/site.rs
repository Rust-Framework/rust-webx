use rust_webapp::*;

use crate::contracts::site::{SiteConfig, SiteInfoRequest};

pub fn load_site_config() -> SiteConfig {
    let path = crate::common::paths::resolve_data_path("appsettings.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
    parsed
        .get("Site")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

#[derive(Default)]
pub struct SiteInfoHandler;

#[handler]
#[async_trait]
impl IRequestHandler<SiteInfoRequest, SiteConfig> for SiteInfoHandler {
    async fn handle(&self, _req: SiteInfoRequest) -> Result<SiteConfig> {
        Ok(load_site_config())
    }
}
