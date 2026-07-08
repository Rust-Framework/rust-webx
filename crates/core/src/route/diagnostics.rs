//! Compile-time route and handler diagnostics.

use std::collections::HashSet;

use crate::route::scan::{HandlerCache, HandlerRegistration, RouteEntry};

/// Snapshot of a registered HTTP route.
#[derive(Debug, Clone)]
pub struct RouteSnapshot {
    pub method: String,
    pub path: &'static str,
    pub request_type: &'static str,
    pub response_type: &'static str,
    pub has_handler: bool,
}

/// Collect all inventory routes and whether a matching `#[handler]` exists.
pub fn route_snapshots() -> Vec<RouteSnapshot> {
    let cache = HandlerCache::build();
    let mut routes: Vec<RouteSnapshot> = inventory::iter::<RouteEntry>()
        .map(|entry| RouteSnapshot {
            method: entry.method.as_str().to_string(),
            path: entry.path,
            request_type: entry.handler_type,
            response_type: entry.rsp_type,
            has_handler: cache.get(entry.handler_type).is_some(),
        })
        .collect();
    routes.sort_by(|a, b| a.path.cmp(b.path).then(a.method.cmp(&b.method)));
    routes
}

/// Request types with a route but no registered handler.
pub fn orphan_routes() -> Vec<&'static str> {
    route_snapshots()
        .into_iter()
        .filter(|r| !r.has_handler)
        .map(|r| r.request_type)
        .collect()
}

/// Request types registered via `#[handler]` without a matching route.
pub fn orphan_handlers() -> Vec<&'static str> {
    let routed: HashSet<&'static str> = inventory::iter::<RouteEntry>()
        .map(|e| e.handler_type)
        .collect();

    let mut orphans = Vec::new();
    for reg in inventory::iter::<HandlerRegistration>() {
        if !routed.contains(reg.req_type_name) {
            orphans.push(reg.req_type_name);
        }
    }
    orphans.sort_unstable();
    orphans
}

/// Human-readable route/handler diagnostic report (no tracing dependency).
pub fn format_route_diagnostics() -> String {
    use std::fmt::Write;

    let routes = route_snapshots();
    let mut out = format!("Registered HTTP routes: {}\n", routes.len());

    for route in &routes {
        let status = if route.has_handler { "ok" } else { "ORPHAN" };
        let _ = writeln!(
            out,
            "  [{status}] {} {}  {} => {}",
            route.method, route.path, route.request_type, route.response_type
        );
    }

    let missing_handlers = orphan_routes();
    if !missing_handlers.is_empty() {
        let _ = writeln!(out, "\nRoutes without #[handler] ({}):", missing_handlers.len());
        for req in missing_handlers {
            let _ = writeln!(out, "  - {req}");
        }
    }

    let missing_routes = orphan_handlers();
    if !missing_routes.is_empty() {
        let _ = writeln!(out, "\nHandlers without route ({}):", missing_routes.len());
        for req in missing_routes {
            let _ = writeln!(out, "  - {req}");
        }
    }

    out
}
