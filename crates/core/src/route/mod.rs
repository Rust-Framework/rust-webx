// rust-webx-core DI extensions — automatic service registration and module scanning.

pub mod binding;
pub mod catalog;
pub mod diagnostics;
pub mod ext;
pub mod scan;

pub use binding::{BindInput, Binder, RequestModel, Source};
pub use catalog::*;
pub use diagnostics::*;
pub use ext::{is_mediator_active, should_scan_endpoints, IServiceCollectionExt};
pub use scan::*;
