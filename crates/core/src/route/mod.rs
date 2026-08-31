// rust-webx-core DI extensions — automatic service registration and module scanning.

pub mod diagnostics;
pub mod ext;
pub mod params;
pub mod scan;

pub use diagnostics::*;
pub use ext::{is_mediator_active, should_scan_endpoints, IServiceCollectionExt};
pub use params::*;
pub use scan::*;
