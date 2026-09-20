//! Host and DI extension methods (`*ServiceCollectionExtensions` / `Use*`).

mod host_builder;
mod service_collection;

pub use host_builder::HostBuilderExt;
pub use service_collection::ServiceCollectionExt;
