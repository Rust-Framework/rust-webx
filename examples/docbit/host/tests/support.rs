//! Test host factory — mirrors `src/main.rs` (Program.cs) for `webx::spawn`.

use docbit_contracts::site::SiteConfig;
use docbit_host::startup::{sqlite_db_options, HostBuilderExt, ServiceCollectionExt};
use webx::*;

// Link the inventory registrations, exactly as `src/main.rs` does.
use docbit_domain as _;
use docbit_handlers as _;

// Integration tests never execute `fn main`; register the build.rs table here.
#[webx::embed_assets]
mod __embedded_assets {}

pub fn build_host() -> Host {
    let db = sqlite_db_options().expect("SQLite preflight must succeed in tests");

    let mut builder = Host::builder()
        .register(move |svc| svc.add_docbit_db(db))
        .register(|svc| svc.add_mediator())
        .add_options::<SiteConfig>("Site")
        .use_authentication()
        .add_memory_cache(CacheOptions::default())
        .use_spa("wwwroot");

    if AppMode::from_env() == AppMode::Production {
        builder = builder.use_production_middleware();
    }

    builder.try_build().expect("test host must build")
}
