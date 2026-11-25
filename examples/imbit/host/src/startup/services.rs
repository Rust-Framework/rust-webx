//! Application services and the WebSocket endpoint.

use std::sync::Arc;

use webx::rust_dix::ServiceCollection;
use webx::{
    CookieOptions, CsrfConfig, FileSystemObjectStore, IObjectStore, IServiceCollectionExt,
    ISessionRepository, InMemoryObjectStore, InMemorySessionRepository, SessionCookieScheme,
};

use imbit_domain::{ImStore, RecordingRegistry};

/// WebSocket endpoint carrying the message-as-request protocol.
pub const WS_PATH: &str = "/ws/im";

/// Object storage: in-memory by default; `IMBIT_STORAGE_DIR` switches to the
/// local-filesystem implementation of the same port an S3 adapter implements.
pub fn object_store() -> Arc<dyn IObjectStore> {
    match std::env::var("IMBIT_STORAGE_DIR") {
        Ok(dir) if !dir.trim().is_empty() => Arc::new(FileSystemObjectStore::new(dir.trim())),
        _ => Arc::new(InMemoryObjectStore::new()),
    }
}

/// Cookie session scheme used by browsers (`POST /api/session`).
///
/// The cookie carries an opaque 256-bit session id (`HttpOnly`, `Secure`,
/// `SameSite=Lax`) resolved through [`ISessionRepository`]; logout/deletion is
/// immediate. In-memory is right for this single-process reference app — a
/// multi-instance deployment registers `EfSessionRepository` (or another
/// shared repository) behind the same port.
pub fn session_cookie_scheme() -> Arc<SessionCookieScheme> {
    let repository: Arc<dyn ISessionRepository> = Arc::new(InMemorySessionRepository::new());
    let scheme = SessionCookieScheme::new(repository, CookieOptions::default())
        .expect("the framework default cookie options are always valid");
    Arc::new(scheme)
}

/// Double-submit CSRF configuration.
///
/// The host requires it as soon as a [`SessionCookieScheme`] is registered; the
/// session-exchange endpoint also uses it to mint the readable CSRF cookie whose
/// value the browser mirrors in `X-CSRF-Token`.
pub fn csrf_config() -> CsrfConfig {
    CsrfConfig::double_submit()
}

/// Register the services the handlers inject.
///
/// Besides the reference repositories this wires the two browser-session
/// services explicitly: the handler that signs a bearer token into a cookie
/// session injects the scheme, and the CSRF config mints the readable token
/// cookie next to the `HttpOnly` one.
pub fn add_imbit_services(
    svc: ServiceCollection,
    storage: Arc<dyn IObjectStore>,
    cookie_scheme: Arc<SessionCookieScheme>,
    csrf: CsrfConfig,
) -> ServiceCollection {
    svc.add_mediator()
        .singleton(|_| Arc::new(ImStore::default()))
        .singleton(|_| Arc::new(RecordingRegistry::default()))
        .try_add_singleton::<dyn IObjectStore>(move |_| Arc::clone(&storage))
        .instance(cookie_scheme)
        .instance(Arc::new(csrf))
}
