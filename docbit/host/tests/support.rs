//! Test host factory — mirrors `src/main.rs` (Program.cs) for `webx::spawn`.

use docbit_contracts::site::SiteConfig;
use docbit_host::startup::{HostBuilderExt, ServiceCollectionExt};
use webx::*;

// Integration tests never execute `fn main`; register the build.rs table here.
#[webx::embed_assets]
mod __embedded_assets {}

pub fn build_host() -> Host {
    let mut builder = Host::builder()
        .register(|svc| svc.add_docbit_db())
        .register(|svc| svc.add_mediator())
        .add_options::<SiteConfig>("Site")
        .add_authentication()
        .use_resource_authorization()
        .add_memory_cache()
        .use_spa("wwwroot");

    if AppMode::from_env() == AppMode::Production {
        builder = builder.use_production_middleware();
    }

    builder.build()
}
