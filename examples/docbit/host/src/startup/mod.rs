//! Application startup — ASP.NET Core `Startup` / extension methods analogue.
//!
//! | Folder | Role |
//! |--------|------|
//! | [`services`] | `Add*` / `Use*` on DI and `HostBuilder`, first-boot provisioning |
//! | [`hosted`] | `IHostedService` implementations |
//! | [`seed`] | One-shot data bootstrap called from hosted services |

pub mod hosted;
pub mod seed;
pub mod services;

pub use hosted::DbInitService;
pub use services::{
    ensure_jwt_secret_file, sqlite_db_options, HostBuilderExt, ServiceCollectionExt,
};
