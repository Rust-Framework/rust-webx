// lrwf-core DI extensions — automatic service registration and module scanning.

pub mod ext;
pub mod scan;

pub use ext::{should_scan_endpoints, is_mediator_active, IServiceCollectionExt};
pub use scan::*;
