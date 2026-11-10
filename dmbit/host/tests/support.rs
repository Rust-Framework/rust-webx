//! Test host factory — mirrors `src/main.rs` for `webx::spawn`.

use dmbit_contracts::site::SiteConfig;
use dmbit_host::startup::{HostBuilderExt, ServiceCollectionExt};
use webx::*;

pub fn build_host() -> Host {
    let mut builder = Host::builder()
        .register(|svc| svc.add_dmbit_db())
        .register(|svc| svc.add_mediator())
        .add_options::<SiteConfig>("Site")
        .add_authentication();

    if AppMode::from_env() == AppMode::Production {
        builder = builder.use_production_middleware();
    }

    builder.build()
}
