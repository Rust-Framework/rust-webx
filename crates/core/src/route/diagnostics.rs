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

/// Routes with inventory entries but no matching `#[handler]` (includes path/method).
pub fn orphan_route_details() -> Vec<RouteSnapshot> {
    route_snapshots()
        .into_iter()
        .filter(|r| !r.has_handler)
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

/// Request types with more than one `#[handler]` registration (last wins at runtime).
pub fn duplicate_handlers() -> Vec<(&'static str, usize)> {
    let mut counts: std::collections::HashMap<&'static str, usize> =
        std::collections::HashMap::new();
    for reg in inventory::iter::<HandlerRegistration>() {
        *counts.entry(reg.req_type_name).or_default() += 1;
    }
    let mut dupes: Vec<_> = counts.into_iter().filter(|(_, count)| *count > 1).collect();
    dupes.sort_by_key(|(name, _)| *name);
    dupes
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

    let orphan_route_rows = orphan_route_details();
    if !orphan_route_rows.is_empty() {
        let _ = writeln!(
            out,
            "\nRoutes without #[handler] ({}):",
            orphan_route_rows.len()
        );
        let _ = writeln!(
            out,
            "  Fix: add #[handler] or #[handler(inject)] impl IRequestHandler<Request, Response>"
        );
        for route in &orphan_route_rows {
            let _ = writeln!(
                out,
                "  - {} {} ({})",
                route.method, route.path, route.request_type
            );
        }
    }

    let missing_routes = orphan_handlers();
    if !missing_routes.is_empty() {
        let _ = writeln!(out, "\nHandlers without route ({}):", missing_routes.len());
        let _ = writeln!(
            out,
            "  Fix: add #[get]/#[post]/... impl IRequest<R> for the request type, or remove the orphan #[handler]"
        );
        for req in &missing_routes {
            let _ = writeln!(out, "  - {req}");
        }
    }

    let dupes = duplicate_handlers();
    if !dupes.is_empty() {
        let _ = writeln!(
            out,
            "\nDuplicate #[handler] registrations ({}); last inventory entry wins:",
            dupes.len()
        );
        let _ = writeln!(
            out,
            "  Fix: keep one #[handler] per request type; remove duplicate impl blocks"
        );
        for (req, count) in &dupes {
            let _ = writeln!(out, "  - {req} ({count} handlers)");
        }
    }

    if orphan_route_rows.is_empty() && missing_routes.is_empty() && dupes.is_empty() {
        let _ = writeln!(
            out,
            "\nRoute table OK — every route has a handler and every handler has a route."
        );
    } else {
        let _ = writeln!(
            out,
            "\nStartup will panic on orphan routes/handlers. Run: cargo run -p <host-crate> -- --doctor"
        );
    }

    out
}
