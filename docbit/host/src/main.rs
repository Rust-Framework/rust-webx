//! Docbit host — ASP.NET Core `Program.cs`.
//!
//! - `build.rs` `web_root` — files baked into the binary (compile-time)
//! - `#[webx::main(embed)]` — opt into that table at the entry point
//! - `.use_spa("wwwroot")` — runtime disk overlay (operator overrides)
//! - [`startup`](docbit_host::startup) — Add* / Use* and hosted services

use docbit_contracts::site::SiteConfig;
use docbit_host::startup::{HostBuilderExt, ServiceCollectionExt};
use webx::*;

#[webx::main(embed)]
async fn main() {
    if std::env::args().any(|a| a == "--doctor") {
        print!("{}", webx::format_route_diagnostics());
        return;
    }

    let mut builder = Host::builder()
        .register(|svc| svc.add_docbit_db())
        .register(|svc| svc.add_mediator())
        .add_options::<SiteConfig>("Site")
        .add_authentication()
        .use_resource_authorization()
        .add_memory_cache()
        .use_spa("wwwroot");

    if AppMode::from_env() == AppMode::Production {
        tracing::info!("[docbit] Production middleware: compression, timing, request-tracing");
        builder = builder.use_production_middleware();
    }

    builder.build().run().await.expect("Server failed");
}
