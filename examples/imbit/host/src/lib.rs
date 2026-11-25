//! imbit host — the composition root (ASP.NET Core `Program.cs` equivalent).
//!
//! | Item | Role |
//! |------|------|
//! | [`build_host`] | host assembly: mode, options, authentication, session store, DI, WebSocket |
//! | [`run`] | build and serve until shutdown |
//! | `main.rs` | process entry: `IMBIT_ADDR` → [`run`] |
//! | [`startup`] | configuration defaults and service registration |
//!
//! No business rule lives here: handlers orchestrate, the domain decides, and
//! the host only wires the framework.

pub mod startup;

use std::sync::Arc;

use webx::*;

use imbit_contracts::realtime::{ImReply, ImRequest};

// Handler registrations are collected by `inventory`; link the crate so the
// linker keeps them even though no host type names a handler.
use imbit_handlers as _;

pub use startup::{DEFAULT_ADDRESS, DEV_JWT_SECRET, WS_PATH};

/// Build the reference host (HTTP + WebSocket + metrics).
///
/// * **Authentication** — `use_authentication()` installs JWT bearer
///   authentication shared by the HTTP routes and the WebSocket upgrade;
///   `use_session_store()` makes `/api/logout` revocation effective in this
///   single-process deployment.
/// * **Services** — mediator, the in-process IM store, the recording index and
///   the object-storage port (`IMBIT_STORAGE_DIR` switches to the filesystem).
/// * **Real time** — the `/ws/im` message-as-request endpoint, dispatched
///   through the same handler pipeline as HTTP.
/// * **Static assets** — the reference app serves no SPA (`no_spa()`); an app
///   with a web client would use `use_spa(...)` here instead.
pub fn build_host() -> Host {
    let mode = startup::config::app_mode();
    let storage = startup::services::object_store();

    Host::builder()
        .mode(mode)
        .no_spa()
        .configure(move |app| {
            app.options(move |options| startup::config::apply_options(options, mode));
        })
        .use_authentication()
        .use_session_store(Arc::new(InMemorySessionStore::new()))
        .register(move |svc| startup::services::add_imbit_services(svc, Arc::clone(&storage)))
        .use_websocket_request::<ImRequest, ImReply>(startup::services::WS_PATH)
        .build()
}

/// Run the host until shutdown.
pub async fn run(address: &str) -> webx::Result<()> {
    build_host().run_at(address).await
}
