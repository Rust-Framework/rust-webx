//! Dmbit host — composition root (`Program.cs` equivalent).
//!
//! | Path | Role |
//! |---|---|
//! | [`startup::services`] | `Add*` / `Use*` registration on DI and `HostBuilder` |
//! | [`startup::hosted`] | `IHostedService` implementations (schema install) |

pub mod startup;

pub use startup::{DbInitService, HostBuilderExt, ServiceCollectionExt};
