//! Docbit host — ASP.NET Core `Program.cs`.
//!
//! - `build.rs` `web_root` — files baked into the binary (compile-time)
//! - `#[webx::main(embed)]` — opt into that table at the entry point
//! - `.use_spa("wwwroot")` — runtime disk overlay (operator overrides)
//! - [`docbit_host::startup`] — `Add*` / `Use*` services and hosted services
//!
//! Every fallible startup step is propagated and reported once, then the process
//! exits non-zero — never a panic in the entry point.

use docbit_contracts::site::SiteConfig;
use docbit_host::startup::{
    ensure_jwt_secret_file, sqlite_db_options, HostBuilderExt, ServiceCollectionExt,
};
use webx::*;

// Force the `inventory` registrations of the other layers into this binary:
// routes/handlers come from `docbit_handlers`, entities from `docbit_domain`.
use docbit_domain as _;
use docbit_handlers as _;

#[webx::main(embed)]
async fn main() {
    if std::env::args().any(|a| a == "--doctor") {
        print!("{}", webx::format_route_diagnostics());
        return;
    }

    if let Err(error) = run().await {
        // Logging is initialised by the host builder, which may not have run yet.
        eprintln!("[docbit] fatal: {error}");
        std::process::exit(1);
    }
}

/// Build and run the host.
///
/// Startup order matters: the signing key file exists before the framework
/// resolves `Jwt.SecretFile`, and the database is preflighted before DI hands a
/// `DbContext` to a request.
async fn run() -> Result<()> {
    let mode = AppMode::from_env();

    ensure_jwt_secret_file(mode)?;

    let db = sqlite_db_options()?;

    let mut builder = Host::builder()
        .register(move |svc| svc.add_docbit_db(db))
        .register(|svc| svc.add_mediator())
        .add_options::<SiteConfig>("Site")
        .use_authentication()
        .add_memory_cache(CacheOptions::default())
        .use_spa("wwwroot");

    if mode == AppMode::Production {
        tracing::info!("[docbit] Production middleware: compression, timing, request-tracing");
        builder = builder.use_production_middleware();
    }

    // `try_build` reports configuration/DI violations instead of panicking.
    builder.try_build()?.run().await
}
