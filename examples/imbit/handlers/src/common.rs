//! Helpers shared by more than one handler.

use std::sync::Arc;

use webx::*;

use imbit_contracts::realtime::ImEvent;

/// Offline catch-up page size (HTTP and WebSocket history share it).
pub(crate) const CATCH_UP_LIMIT: usize = 200;

/// The connection-scoped registry, when the handler runs inside a WebSocket
/// message scope. HTTP requests and plain unit tests have none, so fan-out and
/// presence degrade to a no-op instead of failing.
pub(crate) fn connection_registry() -> Option<Arc<dyn IConnectionRegistry>> {
    RequestContext::connection()?.resolve::<dyn IConnectionRegistry>()
}

/// Encode an outbound event frame.
///
/// An unencodable frame is a server defect, so its text stays log-only (`500`)
/// instead of reaching the client.
pub(crate) fn encode_event(event: &ImEvent) -> Result<String> {
    serde_json::to_string(event)
        .map_err(|error| Error::internal(format!("websocket event encode failed: {error}")))
}
