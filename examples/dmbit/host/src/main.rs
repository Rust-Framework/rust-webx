//! Dmbit host entry point.
//!
//! `main` only composes the host and runs it: the handler crate is linked for
//! its `inventory` route registrations, startup does the DI/middleware wiring,
//! and a startup failure is printed and turned into a non-zero exit instead of a
//! panic.

// Route metadata lives in `inventory` statics, which the linker keeps only when
// the crate is referenced from the binary entry point.
use dmbit_handlers as _;

use dmbit_contracts::site::SiteConfig;
use dmbit_host::startup::{HostBuilderExt, ServiceCollectionExt};
use webx::*;

#[webx::main]
async fn main() {
    if std::env::args().any(|a| a == "--doctor") {
        print!("{}", webx::format_route_diagnostics());
        return;
    }

    let mut builder = Host::builder()
        .register(|svc| svc.add_dmbit_db())
        .register(|svc| svc.add_mediator())
        .add_options::<SiteConfig>("Site")
        .use_authentication();

    if AppMode::from_env() == AppMode::Production {
        tracing::info!("[dmbit] Production middleware: compression, timing, request-tracing");
        builder = builder.use_production_middleware();
    }

    let host = match builder.try_build() {
        Ok(host) => host,
        Err(e) => {
            tracing::error!("[dmbit] host build failed: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = host.run().await {
        tracing::error!("[dmbit] server stopped with an error: {e}");
        std::process::exit(1);
    }
}
