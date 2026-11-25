//! Application startup — `Startup` / extension-method analogue.
//!
//! | Folder | Role |
//! |--------|------|
//! | [`services`] | `Add*` / `Use*` on DI and `HostBuilder` |
//! | [`hosted`] | `IHostedService` implementations |

pub mod hosted;
pub mod services;

pub use hosted::DbInitService;
pub use services::{HostBuilderExt, ServiceCollectionExt};
