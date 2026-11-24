//! Application startup — ASP.NET Core `Startup` / extension methods analogue.
//!
//! | Folder | Role |
//! |--------|------|
//! | [`extensions`] | `Add*` / `Use*` on DI and `HostBuilder` |
//! | [`hosted`] | `IHostedService` implementations |

pub mod extensions;
pub mod hosted;

pub use extensions::{HostBuilderExt, ServiceCollectionExt};
pub use hosted::DbInitService;
