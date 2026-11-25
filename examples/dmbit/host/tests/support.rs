//! Test host factory — mirrors `src/main.rs` for `webx::spawn`.

// The route metadata lives in `inventory` statics, so the test binary has to
// reference the handler crate explicitly; otherwise the linker drops it and
// every endpoint answers 404.
use dmbit_handlers as _;

use dmbit_contracts::site::SiteConfig;
use dmbit_host::startup::{HostBuilderExt, ServiceCollectionExt};
use webx::*;

pub fn build_host() -> Host {
    let mut builder = Host::builder()
        .register(|svc| svc.add_dmbit_db())
        .register(|svc| svc.add_mediator())
        .add_options::<SiteConfig>("Site")
        .use_authentication();

    if AppMode::from_env() == AppMode::Production {
        builder = builder.use_production_middleware();
    }

    builder.build()
}
