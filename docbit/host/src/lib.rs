//! Docbit host — composition root (library + binary).
//!
//! Exports `build_host()` for integration tests and the `docbit-host` binary.

mod startup;

use docbit_contracts::site::SiteConfig;
use docbit_domain::prepare_context;
use rust_ef_mysql::DbContextOptionsBuilderExt as _;
use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
use rust_webx::rust_dix::ServiceCollection;
use rust_webx::*;
use rust_webx_ef::{EfServiceCollectionExt, SaveChangesLogInterceptor};

extern crate docbit_domain;
extern crate docbit_handlers;

pub use startup::DbInitService;

/// Build the docbit [`Host`] with standard DI, auth, cache, and mode-specific middleware.
pub fn build_host() -> Host {
    let mode = AppMode::from_env();
    let mut builder = Host::builder()
        .register(|svc| register_db_context(svc))
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

/// Register DbContext (SQLite in Development, MySQL in Production).
pub fn register_db_context(svc: ServiceCollection) -> ServiceCollection {
    svc.add_ef_dbcontext(
        |opts| {
            opts.add_interceptor(SaveChangesLogInterceptor);
            match AppMode::from_env() {
                AppMode::Production => {
                    let cs = std::env::var("DATABASE_URL")
                        .expect("DATABASE_URL environment variable required in Production");
                    opts.use_mysql(&cs);
                    tracing::info!("[docbit] DbContext provider: MySQL");
                }
                AppMode::Development => {
                    let path = app_base().join("app.db");
                    opts.use_sqlite(&path.to_string_lossy());
                    tracing::info!("[docbit] SQLite path: {}", path.display());
                }
            }
        },
        |ctx| prepare_context(ctx),
    )
}
