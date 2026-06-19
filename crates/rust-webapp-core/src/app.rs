//! Application layer traits: IApplicationBuilder and IHost.

use crate::error::Result;
use crate::middleware::IMiddleware;
use std::sync::Arc;

/// Builder for configuring the middleware pipeline.
///
/// Analogous to ASP.NET Core's IApplicationBuilder.
pub trait IApplicationBuilder: Send + Sync {
    fn use_middleware<T: IMiddleware + 'static>(&mut self) -> &mut Self;
    fn use_router(&mut self) -> &mut Self;
    fn build(self: Arc<Self>) -> Box<dyn IHost>;
}

/// The web host that binds to an address and starts serving HTTP requests.
///
/// Analogous to ASP.NET Core's IWebHost.
#[async_trait::async_trait]
pub trait IHost: Send + Sync {
    async fn run(&self, addr: &str) -> Result<()>;
    async fn stop(&self) -> Result<()>;
}
