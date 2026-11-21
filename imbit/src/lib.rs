//! IM reference application (2.3-7).
//!
//! End-to-end capabilities exercised:
//!
//! * **HTTP** — login through the framework ports (`ITokenIssuer` /
//!   `IPasswordHasher`), room create/list/join, offline history catch-up.
//! * **WebSocket** — message-as-request (`use_websocket_request`): join, send
//!   (idempotent by `client_msg_id`), history, presence; room fan-out through
//!   `IConnectionRegistry`; upgrade authenticated by the normal HTTP pipeline
//!   (the JWT issued by the login endpoint is presented in `Authorization`).
//! * **Reconnect** — the client resends unacknowledged/failed messages with the
//!   same `client_msg_id` (deduplicated server-side) and pulls the gap with
//!   `history { after_seq }`.

pub mod contracts;
pub mod handlers;
pub mod recording;
pub mod store;

use std::sync::Arc;

use webx::{AppMode, Host, IObjectStore, IServiceCollectionExt};

use recording::RecordingRegistry;
use store::ImStore;

/// Development JWT secret (32+ bytes). Production supplies `JWT_SECRET` or
/// `Jwt.SecretFile` instead of hard-coding one.
pub const DEV_JWT_SECRET: &str = "imbit-development-secret-change-me-0123456789";

/// Development defaults for the framework `Turn` configuration section.
///
/// The section itself is the single source of truth (`Turn.Secret`,
/// `Turn.Urls`, `APP__Turn__*`); the `TURN_*` environment variables below are a
/// convenience for this reference app so `cargo run -p imbit` works without an
/// appsettings file. Values already present in the section are never overridden.
fn configure_turn_defaults(options: &mut webx::AppOptions) {
    if options.turn.secret.trim().is_empty() {
        options.turn.secret = std::env::var("TURN_SECRET")
            .unwrap_or_else(|_| "imbit-turn-dev-secret-change-me".to_string());
    }
    if options.turn.urls.is_empty() {
        let urls = std::env::var("TURN_URLS")
            .unwrap_or_else(|_| "turn:127.0.0.1:3478,stun:127.0.0.1:3478".to_string());
        options.turn.urls = urls
            .split(',')
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_string)
            .collect();
    }
    if options.turn.ttl_seconds == webx::DEFAULT_TURN_TTL_SECONDS {
        if let Some(ttl) = std::env::var("TURN_TTL_SECONDS")
            .ok()
            .and_then(|value| value.parse().ok())
        {
            options.turn.ttl_seconds = ttl;
        }
    }
}

/// Object storage: in-memory by default; `IMBIT_STORAGE_DIR` switches to the
/// real local-filesystem implementation (same port an S3 adapter implements).
fn object_store() -> Arc<dyn IObjectStore> {
    match std::env::var("IMBIT_STORAGE_DIR") {
        Ok(dir) if !dir.trim().is_empty() => Arc::new(webx::FileSystemObjectStore::new(dir.trim())),
        _ => Arc::new(webx::InMemoryObjectStore::new()),
    }
}

/// Build the reference host (HTTP + WebSocket + metrics).
///
/// TURN credentials come from the framework `Turn` configuration section; the
/// host validates it at startup and registers [`webx::TurnRestConfig`] for the
/// `/api/turn` handler to inject.
pub fn build_host() -> Host {
    let storage = object_store();
    Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.useOptions(|options| {
                options.jwt.secret = DEV_JWT_SECRET.to_string();
                options.metrics.enabled = true;
                configure_turn_defaults(options);
            });
        })
        .add_authentication()
        .register(move |svc| {
            let storage = Arc::clone(&storage);
            svc.add_mediator()
                .singleton(|_| Arc::new(ImStore::default()))
                .singleton(|_| Arc::new(RecordingRegistry::default()))
                .try_add_singleton::<dyn IObjectStore>(move |_| Arc::clone(&storage))
        })
        .use_websocket_request::<contracts::ImRequest, contracts::ImReply>("/ws/im")
        .build()
}

/// Run until shutdown.
pub async fn run(address: &str) -> webx::Result<()> {
    build_host().run_at(address).await
}
