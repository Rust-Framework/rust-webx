//! Application services and the WebSocket endpoint.

use std::sync::Arc;

use webx::rust_dix::ServiceCollection;
use webx::{FileSystemObjectStore, IObjectStore, IServiceCollectionExt, InMemoryObjectStore};

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

/// Register the services the handlers inject.
pub fn add_imbit_services(
    svc: ServiceCollection,
    storage: Arc<dyn IObjectStore>,
) -> ServiceCollection {
    svc.add_mediator()
        .singleton(|_| Arc::new(ImStore::default()))
        .singleton(|_| Arc::new(RecordingRegistry::default()))
        .try_add_singleton::<dyn IObjectStore>(move |_| Arc::clone(&storage))
}
