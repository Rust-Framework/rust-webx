//! Application startup — ASP.NET Core `Startup` / extension methods analogue.
//!
//! | Folder | Role |
//! |--------|------|
//! | [`extensions`] | `Add*` / `Use*` on DI and `HostBuilder` |
//! | [`hosted`] | `IHostedService` implementations |
//! | [`seed`] | One-shot data bootstrap called from hosted services |

pub mod extensions;
pub mod hosted;
pub mod seed;

pub use extensions::{HostBuilderExt, ServiceCollectionExt};
pub use hosted::DbInitService;
