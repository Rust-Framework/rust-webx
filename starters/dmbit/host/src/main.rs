//! Dmbit host — ASP.NET Core `Program.cs`.
//!
//! `startup/` holds Add* / Use* extensions and hosted services.

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
        .add_authentication();

    if AppMode::from_env() == AppMode::Production {
        tracing::info!("[dmbit] Production middleware: compression, timing, request-tracing");
        builder = builder.use_production_middleware();
    }

    builder.build().run().await.expect("Server failed");
}
