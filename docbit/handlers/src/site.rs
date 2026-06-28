//! Site info handler — returns the loaded `SiteConfig` directly.
//!
//! `SiteConfig` 由 host crate 在启动时从 appsettings.json 读取并以
//! `Arc<SiteConfig>` 注册到 DI 容器；此处仅做只读注入。

use std::sync::Arc;

use rust_webapp::*;

use docbit_contracts::site::{SiteConfig, SiteInfoRequest};

#[rust_dicore::inject]
pub struct SiteInfoHandler {
    config: Arc<SiteConfig>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<SiteInfoRequest, SiteConfig> for SiteInfoHandler {
    async fn handle(&self, _: SiteInfoRequest) -> Result<SiteConfig> {
        Ok((*self.config).clone())
    }
}
