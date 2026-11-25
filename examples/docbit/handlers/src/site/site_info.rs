//! `GET /api/site` — the loaded `SiteConfig`, straight from DI.
//!
//! The host reads the `Site` section from appsettings at startup and registers
//! `Arc<SiteConfig>`; this handler only injects it.

use std::sync::Arc;

use webx::*;

use docbit_contracts::site::{SiteConfig, SiteInfoRequest};

#[derive(Inject)]
pub struct SiteInfoHandler {
    #[inject]
    config: Arc<SiteConfig>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<SiteInfoRequest, SiteConfig> for SiteInfoHandler {
    async fn handle(&mut self, _: SiteInfoRequest) -> ActionResult<SiteConfig> {
        Ok((*self.config).clone())
    }
}
