//! Application layer trait: IHost.

use crate::error::Result;

/// The web host that binds to an address and starts serving HTTP requests.
///
/// Analogous to ASP.NET Core's IWebHost.
#[async_trait::async_trait]
pub trait IHost: Send + Sync {
    async fn run(&self, addr: &str) -> Result<()>;
    async fn stop(&self) -> Result<()>;
}
