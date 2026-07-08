#![allow(non_snake_case)] // rust-dix #[derive(Inject)] generates __rdi_construct_* symbols

//! Docbit host — composition root (library + binary).
//!
//! Exports `build_host()` for integration tests and the `docbit-host` binary.

mod startup;

use std::sync::Arc;

use docbit_contracts::site::SiteConfig;
use docbit_domain::prepare_context;
use rust_ef::db_context::{DbContext, DbContextOptionsBuilder};
use rust_ef_mysql::DbContextOptionsBuilderExt as _;
use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
use rust_webx::rust_dix::ServiceCollection;
use rust_webx::*;

extern crate docbit_domain;
extern crate docbit_handlers;

pub use startup::DbInitService;

/// Build the docbit [`Host`] with standard DI, auth, cache, and mode-specific middleware.
pub fn build_host() -> Host {
    let mode = AppMode::from_env();
    let mut builder = Host::builder()
        .register(|svc| register_db_context(svc))
        .register(|svc| svc.add_mediator())
        .add_options::<SiteConfig>("Site")
        .add_authentication()
        .add_memory_cache();

    if mode == AppMode::Production {
        tracing::info!("[docbit] Production middleware: compression, timing, request-tracing");
        builder = builder
            .use_middleware::<CompressionMiddleware>()
            .use_middleware::<TimingMiddleware>()
            .use_middleware::<RequestTracing>();
    }

    builder.build()
}

/// Register scoped `DbContext` (SQLite in Development, MySQL in Production).
pub fn register_db_context(svc: ServiceCollection) -> ServiceCollection {
    let mut builder = DbContextOptionsBuilder::new();
    match AppMode::from_env() {
        AppMode::Production => {
            let cs = std::env::var("DATABASE_URL")
                .expect("DATABASE_URL environment variable required in Production");
            builder.use_mysql(&cs);
            tracing::info!("[docbit] DbContext provider: MySQL");
        }
        AppMode::Development => {
            let path = app_base().join("app.db");
            builder.use_sqlite(&path.to_string_lossy());
            tracing::info!("[docbit] SQLite path: {}", path.display());
        }
    }

    let options = Arc::new(builder.build());
    options
        .create_provider()
        .expect("DbContext provider initialization failed at startup");

    svc.scoped(move |_| {
        let mut ctx = DbContext::from_options(&options).expect("Failed to create DbContext");
        prepare_context(&mut ctx);
        Arc::new(ctx)
    })
}
