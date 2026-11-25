//! Composition-root services — `Add*` / `Use*` on DI and `HostBuilder`, plus the
//! startup provisioning that must happen before the host is built.
//!
//! | File | Role |
//! |------|------|
//! | [`host_builder`] | `Use*` pipeline helpers (`MiddlewareSpec`) |
//! | [`service_collection`] | `AddDbContext`-style SQLite registration |
//! | [`jwt_key`] | first-boot `Jwt.SecretFile` provisioning |

mod host_builder;
mod jwt_key;
mod service_collection;

pub use host_builder::HostBuilderExt;
pub use jwt_key::ensure_jwt_secret_file;
pub use service_collection::{sqlite_db_options, ServiceCollectionExt};
