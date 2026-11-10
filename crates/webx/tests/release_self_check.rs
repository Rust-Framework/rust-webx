//! Release self-check: the compile-time route table has no orphans.
//!
//! `HostBuilder::build()` panics on orphan routes/handlers, but that only runs
//! when a host is actually built in the process. This test checks the same
//! invariants for the umbrella crate's test binary and fails with the full
//! diagnostic table, so a release build catches a forgotten `#[handler]` or a
//! route without an `impl IRequest` even if no test spawns a host.

#[test]
fn route_table_has_no_orphans_or_duplicates() {
    let orphan_routes = webx::orphan_routes();
    let orphan_handlers = webx::orphan_handlers();
    let duplicates = webx::duplicate_handlers();

    assert!(
        orphan_routes.is_empty(),
        "orphan routes (route declared without a matching handler): {orphan_routes:?}\n\n{}",
        webx::format_route_diagnostics()
    );
    assert!(
        orphan_handlers.is_empty(),
        "orphan handlers (handler declared without a route): {orphan_handlers:?}\n\n{}",
        webx::format_route_diagnostics()
    );
    assert!(
        duplicates.is_empty(),
        "duplicate handlers: {duplicates:?}\n\n{}",
        webx::format_route_diagnostics()
    );
}

/// The endpoint catalog is the permission/menu source of truth; every route
/// macro must have produced an entry (health/openapi/SPA are not catalogued).
#[test]
fn route_snapshots_are_consistent() {
    let snapshots = webx::route_snapshots();
    for snapshot in &snapshots {
        assert!(
            snapshot.path.starts_with('/'),
            "route path must be absolute: {} {}",
            snapshot.method,
            snapshot.path
        );
        assert!(
            !snapshot.request_type.is_empty(),
            "route {} {} has no request type",
            snapshot.method,
            snapshot.path
        );
        assert!(
            snapshot.has_handler,
            "route {} {} ({}) has no handler",
            snapshot.method, snapshot.path, snapshot.request_type
        );
    }
}
