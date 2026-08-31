//! Route/handler startup diagnostics (uses tracing — host layer only).

use rust_webx_core::route::diagnostics::{
    duplicate_handlers, orphan_handlers, orphan_route_details, route_snapshots,
};
use rust_webx_core::route::scan::{RouteDispatch, RouteEntry};

/// Fail fast when route/handler configuration is inconsistent.
///
/// Panics at `HostBuilder::build()` when:
/// - a route exists without a matching `#[handler]` registration
/// - a `#[handler]` exists without a matching route
/// - a route has a handler but no generated `RouteDispatch` bridge
pub fn assert_route_configuration_valid() {
    let orphan_route_rows = orphan_route_details();
    let missing_routes = orphan_handlers();

    let mut missing_dispatch: Vec<(&'static str, &'static str, &'static str)> = Vec::new();
    let cache = rust_webx_core::route::scan::HandlerCache::build();
    let dispatch_types: std::collections::HashSet<&'static str> = inventory::iter::<RouteDispatch>()
        .map(|d| d.handler_type)
        .collect();

    for entry in inventory::iter::<RouteEntry> {
        if cache.get(entry.handler_type).is_some() && !dispatch_types.contains(entry.handler_type) {
            missing_dispatch.push((
                entry.method.as_str(),
                entry.path,
                entry.handler_type,
            ));
        }
    }

    if orphan_route_rows.is_empty() && missing_routes.is_empty() && missing_dispatch.is_empty() {
        return;
    }

    let mut message = String::from("Route configuration errors:\n");
    if !orphan_route_rows.is_empty() {
        message.push_str("\nRoutes without #[handler]:\n");
        message.push_str(
            "  Fix: add #[handler] or #[handler(inject)] impl IRequestHandler<Request, Response>\n",
        );
        for route in &orphan_route_rows {
            message.push_str(&format!(
                "  - {} {} ({})\n",
                route.method, route.path, route.request_type
            ));
        }
    }
    if !missing_routes.is_empty() {
        message.push_str("\nHandlers without route:\n");
        message.push_str(
            "  Fix: add #[get]/#[post]/... impl IRequest<R> for the request type, or remove the orphan #[handler]\n",
        );
        for req in &missing_routes {
            message.push_str(&format!("  - {req}\n"));
        }
    }
    if !missing_dispatch.is_empty() {
        message.push_str("\nRoutes with handler but no RouteDispatch:\n");
        message.push_str(
            "  Fix: ensure the request type has a route macro (#[get]/#[post]/...) on its IRequest impl\n",
        );
        for (method, path, req) in &missing_dispatch {
            message.push_str(&format!("  - {method} {path} ({req})\n"));
        }
    }
    message.push_str("\nRun: cargo run -p <host-crate> -- --doctor");
    panic!("{message}");
}

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
                "route has no #[handler] registration — add #[handler] on IRequestHandler impl"
            );
        }
    }

    for handler in orphan_handlers() {
        tracing::warn!(
            request = handler,
            "handler has no matching route — add #[get]/#[post] on IRequest impl or remove #[handler]"
        );
    }

    for (handler, count) in duplicate_handlers() {
        tracing::warn!(
            request = handler,
            count,
            "duplicate #[handler] registration; last inventory entry wins"
        );
    }
}
