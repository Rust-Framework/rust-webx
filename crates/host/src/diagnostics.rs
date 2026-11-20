//! Route/handler startup diagnostics (uses tracing — host layer only).

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use webx_core::route::diagnostics::{
    duplicate_handlers, orphan_handlers, orphan_route_details, route_snapshots,
};
use webx_core::route::scan::{
    HandlerCache, HandlerRegistration, RouteDispatch, RouteDispatchById, RouteEntry,
};

/// Fail fast when route/handler configuration is inconsistent.
///
/// Panics at `HostBuilder::build()` when:
/// - a route exists without a matching `#[handler]` registration
/// - a route has a handler but no generated dispatch bridge
/// - two routes share `(method, path)`, or two handlers share a request `TypeId`
///
/// A handler without a route is valid: it is an `IMediator`-only handler and
/// is reported only through debug diagnostics/`--doctor`.
///
/// `HostBuilder::try_build()` calls [`validate_route_configuration`] instead, so
/// it can report the same problem as an `Err`.
pub fn assert_route_configuration_valid() {
    if let Err(error) = validate_route_configuration() {
        // Keep the historical panic text: the raw report, no `Error` prefix.
        let message = match error {
            webx_core::error::Error::Di(message) => message,
            other => other.to_string(),
        };
        panic!("{message}");
    }
}

/// Validate route/handler consistency, returning the same report as
/// [`assert_route_configuration_valid`] would panic with.
pub fn validate_route_configuration() -> webx_core::error::Result<()> {
    let orphan_route_rows = orphan_route_details();
    let missing_routes = orphan_handlers();

    let mut missing_dispatch: Vec<(&'static str, &'static str, &'static str)> = Vec::new();
    let cache = HandlerCache::build();
    let dispatch_ids: HashSet<TypeId> = inventory::iter::<RouteDispatchById>()
        .map(|d| d.req_type_id)
        .collect();
    let dispatch_names: HashSet<&'static str> = inventory::iter::<RouteDispatch>()
        .map(|d| d.handler_type)
        .collect();

    for entry in inventory::iter::<RouteEntry> {
        let handled = cache.get_by_type_id(entry.req_type_id).is_some()
            || cache.get(entry.handler_type).is_some();
        // A route produced by the endpoint macro has both identities. A
        // hand-written fixture (RouteEntry::new + RouteDispatch) only has the
        // name, so keep the name fallback for that compatibility path.
        let dispatched = dispatch_ids.contains(&entry.req_type_id)
            || dispatch_names.contains(entry.handler_type);
        if handled && !dispatched {
            missing_dispatch.push((entry.method.as_str(), entry.path, entry.handler_type));
        }
    }

    let mut duplicate_routes: Vec<(&'static str, &'static str)> = Vec::new();
    let mut route_keys: HashMap<(&'static str, &'static str), usize> = HashMap::new();
    for entry in inventory::iter::<RouteEntry> {
        *route_keys
            .entry((entry.method.as_str(), entry.path))
            .or_default() += 1;
    }
    for ((method, path), count) in route_keys {
        if count > 1 {
            duplicate_routes.push((method, path));
        }
    }
    duplicate_routes.sort_unstable();

    let mut duplicate_dispatches: Vec<&'static str> = Vec::new();
    let mut dispatch_key_counts: HashMap<TypeId, (&'static str, usize)> = HashMap::new();
    for dispatch in inventory::iter::<RouteDispatchById> {
        let entry = dispatch_key_counts
            .entry(dispatch.req_type_id)
            .or_insert((dispatch.handler_type, 0));
        entry.1 += 1;
    }
    for (_, (name, count)) in dispatch_key_counts {
        if count > 1 {
            duplicate_dispatches.push(name);
        }
    }
    duplicate_dispatches.sort_unstable();

    let duplicate_handler_ids: Vec<&'static str> = {
        let mut counts: HashMap<TypeId, (&'static str, usize)> = HashMap::new();
        for reg in inventory::iter::<HandlerRegistration>() {
            let entry = counts
                .entry(reg.req_type_id)
                .or_insert((reg.req_type_name, 0));
            entry.1 += 1;
        }
        let mut dupes: Vec<&'static str> = counts
            .into_iter()
            .filter(|(_, (_, count))| *count > 1)
            .map(|(_, (name, _))| name)
            .collect();
        dupes.sort_unstable();
        dupes
    };

    // A handler without a route is a valid mediator-only handler: HTTP-ness is
    // determined by the Request's route, not by an annotation on the handler.
    // Keep the list for `--doctor`/debug diagnostics, but do not fail startup.
    for request in &missing_routes {
        tracing::debug!(
            request,
            "handler has no HTTP route; treating it as mediator-only"
        );
    }

    if orphan_route_rows.is_empty()
        && missing_dispatch.is_empty()
        && duplicate_routes.is_empty()
        && duplicate_dispatches.is_empty()
        && duplicate_handler_ids.is_empty()
    {
        return Ok(());
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
    if !missing_dispatch.is_empty() {
        message.push_str("\nRoutes with handler but no dispatch bridge:\n");
        message.push_str(
            "  Fix: ensure the request type has a route macro (#[get]/#[post]/...) on its IRequest impl\n",
        );
        for (method, path, req) in &missing_dispatch {
            message.push_str(&format!("  - {method} {path} ({req})\n"));
        }
    }
    if !duplicate_routes.is_empty() {
        message.push_str("\nDuplicate routes (same method and path):\n");
        message.push_str(
            "  Fix: keep one endpoint per method/path; the router would silently drop the later one\n",
        );
        for (method, path) in &duplicate_routes {
            message.push_str(&format!("  - {method} {path}\n"));
        }
    }
    if !duplicate_dispatches.is_empty() {
        message.push_str("\nDuplicate dispatch bridges for one request type:\n");
        message.push_str("  Fix: keep one route macro per request type\n");
        for name in &duplicate_dispatches {
            message.push_str(&format!("  - {name}\n"));
        }
    }
    if !duplicate_handler_ids.is_empty() {
        message.push_str("\nDuplicate #[handler] registrations for one request type:\n");
        message.push_str("  Fix: keep one #[handler] impl per request type\n");
        for name in &duplicate_handler_ids {
            message.push_str(&format!("  - {name}\n"));
        }
    }
    message.push_str("\nRun: cargo run -p <host-crate> -- --doctor");
    Err(webx_core::error::Error::Di(message))
}

/// Log the route table; route-less (mediator-only) handlers are debug-level.
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
        tracing::debug!(
            request = handler,
            "handler has no matching route; it is available to IMediator only"
        );
    }

    for (handler, count) in duplicate_handlers() {
        tracing::warn!(
            request = handler,
            count,
            "duplicate #[handler] registration for one request type; the build will fail"
        );
    }
}
