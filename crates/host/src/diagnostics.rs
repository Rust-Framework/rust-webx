//! Route/handler startup diagnostics (uses tracing — host layer only).

use rust_webx_core::route::diagnostics::{orphan_handlers, route_snapshots};

/// Log route table and warn on orphan routes/handlers.
pub fn log_startup_diagnostics() {
    let routes = route_snapshots();
    tracing::info!(count = routes.len(), "Registered HTTP routes");
    for route in &routes {
        if route.has_handler {
            tracing::debug!(
                method = %route.method,
                path = route.path,
                request = route.request_type,
                response = route.response_type,
                "route"
            );
        } else {
            tracing::warn!(
                method = %route.method,
                path = route.path,
                request = route.request_type,
                "route has no #[handler] registration"
            );
        }
    }

    for handler in orphan_handlers() {
        tracing::warn!(request = handler, "handler has no matching route");
    }
}
