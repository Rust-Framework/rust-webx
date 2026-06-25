use rust_webapp::*;

use crate::contracts::site::*;
use crate::services::site::SiteConfig;

#[derive(Default)]
pub struct SiteInfoHandler;

#[handler]
#[async_trait]
impl IRequestHandler<SiteInfoRequest, SiteConfig> for SiteInfoHandler {
    async fn handle(&self, _req: SiteInfoRequest) -> Result<SiteConfig> {
        Ok(crate::services::site::load_site_config())
    }
}
