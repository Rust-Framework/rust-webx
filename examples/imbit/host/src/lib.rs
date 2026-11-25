//! imbit host — the composition root (ASP.NET Core `Program.cs` equivalent).
//!
//! | Item | Role |
//! |------|------|
//! | [`build_host`] | host assembly: mode, options, authentication, browser session, DI, WebSocket |
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
/// * **Browser sessions** — a `SessionCookieScheme` plus double-submit CSRF is
///   registered next to bearer authentication. `POST /api/session` exchanges a
///   bearer token for the `HttpOnly` session cookie; the browser then upgrades
///   the WebSocket with the cookie and no `Authorization` header. Bearer clients
///   are untouched: the cookie scheme passes on a request that carries
///   `Authorization`.
/// * **WebSocket origins** — `IMBIT_ORIGINS` (comma-separated) turns the
///   handshake Origin allow-list on; unset keeps it off so non-browser clients
///   (which send no `Origin`) keep working.
/// * **Live-session revocation** — `with_session_revalidation(...)` re-checks the
///   upgrade credential on `IMBIT_REVALIDATION_MS` (default 500 ms) and closes
///   the socket with `1008 SessionRevoked` after logout / session deletion.
/// * **Real-time state** — `use_online_presence(...)` bridges this node's
///   first/last connection transitions into the shared `user:<id>` presence
///   scopes (the `presence` command filters room members through them); room and
///   user fan-out go through the event transport (`RealtimeFanoutEvent` /
///   `RealtimeUserFanoutEvent`).
/// * **Services** — mediator, the in-process IM store, the recording index and
///   the object-storage port (`IMBIT_STORAGE_DIR` switches to the filesystem).
/// * **Real time** — the `/ws/im` message-as-request endpoint, dispatched
///   through the same handler pipeline as HTTP.
/// * **Static assets** — the reference app serves no SPA (`no_spa()`); an app
///   with a web client would use `use_spa(...)` here instead.
pub fn build_host() -> Host {
    let mode = startup::config::app_mode();
    let storage = startup::services::object_store();
    let cookie_scheme = startup::services::session_cookie_scheme();
    let auth_scheme: Arc<dyn IAuthenticationScheme> = cookie_scheme.clone();
    let csrf = startup::services::csrf_config();

    let builder = Host::builder()
        .mode(mode)
        .no_spa()
        .configure(move |app| {
            app.options(move |options| startup::config::apply_options(options, mode));
        })
        .use_authentication()
        .add_authentication_scheme(auth_scheme)
        .use_csrf(csrf.clone())
        .use_session_store(Arc::new(InMemorySessionStore::new()))
        .use_websocket_limits(
            WebSocketLimits::default()
                .with_session_revalidation(startup::config::session_revalidation_interval()),
        )
        .use_online_presence(Arc::new(InMemoryOnlinePresence::default()))
        .register(move |svc| {
            startup::services::add_imbit_services(svc, Arc::clone(&storage), cookie_scheme, csrf)
        })
        .use_websocket_request::<ImRequest, ImReply>(startup::services::WS_PATH);

    // Empty list = no Origin check (non-browser clients keep working). Only an
    // explicit, non-empty `IMBIT_ORIGINS` installs the allow-list.
    let builder = match startup::config::websocket_origins() {
        Some(origins) => builder.use_websocket_origins(origins),
        None => builder,
    };

    builder.build()
}

/// Run the host until shutdown.
pub async fn run(address: &str) -> webx::Result<()> {
    build_host().run_at(address).await
}
