// lrwf-core DI extensions — automatic service registration and module scanning.

pub mod ext;
pub mod scan;

pub use ext::{is_mediator_active, should_scan_endpoints, IServiceCollectionExt};
pub use scan::*;
