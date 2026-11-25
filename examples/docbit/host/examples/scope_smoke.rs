//! Per-request DI scope smoke test — a runnable demonstration of rust-dix scopes.
//!
//! Builds a real [`Host`] (so the framework, not this example, owns logging and
//! the container), then mirrors the dispatch path in
//! `crates/macros/src/endpoint.rs`:
//!
//! 1. `DbContext` is registered **Scoped** (`add_dbcontext` in-memory here;
//!    [`docbit_host::startup::ServiceCollectionExt::add_docbit_db`] does the same
//!    for the real database)
//! 2. per "request": `provider.create_scope()` → `scope.get_owned::<DbContext>()`
//!    → handle → drop the scope
//!
//! Checked here:
//!
//! - the same scope resolves the same `Arc<DbContext>` (scoped cache)
//! - different scopes get different instances (per-request isolation)
//! - the **root** provider refuses a Scoped service (`ScopedFromRootProvider`,
//!   rust-dix strict-scope validation) — a request scope is mandatory
//! - `get_owned::<DbContext>()` always yields a fresh owned instance (the
//!   `#[inject(owned)] ctx: DbContext` path: per-request unit of work)
//! - dropping the scope releases the instance the scope cache held
//!
//! Run with `cargo run --example scope_smoke -p docbit-host`.

use std::sync::Arc;

use rust_dix::ScopeFactory;
use rust_ef::db_context::DbContext;
use rust_ef::di::DbContextServiceCollectionExt as _;
use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
use webx::*;

fn build_provider() -> Result<Host> {
    Host::builder()
        // A demo must not be held to the Production configuration guards.
        .mode(AppMode::Development)
        .register(|svc| {
            svc.add_dbcontext(|opts| {
                opts.use_sqlite_in_memory();
            })
        })
        .try_build()
}

/// Report one assertion; a failure is a returned error, never a panic.
fn check(label: &str, ok: bool) -> Result<()> {
    if !ok {
        return Err(Error::internal(format!("assertion failed: {label}")));
    }
    tracing::info!("[scope_smoke] ok: {label}");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let host = build_provider()?;
    let provider = host.provider();

    tracing::info!("[scope_smoke] per-request DI scope smoke test");
    tracing::info!("[scope_smoke] provider built with add_dbcontext (DbContext = Scoped)");

    // ── Request 1: one scope, two resolutions ──
    let scope1 = provider.create_scope();
    let ctx1_a: Arc<DbContext> = scope1
        .get()
        .map_err(|e| Error::internal(format!("DbContext not registered: {e}")))?;
    let ctx1_b: Arc<DbContext> = scope1
        .get()
        .map_err(|e| Error::internal(format!("DbContext not registered: {e}")))?;
    check(
        "same scope returns the same instance (scoped cache)",
        Arc::ptr_eq(&ctx1_a, &ctx1_b),
    )?;
    let refs_while_scoped = Arc::strong_count(&ctx1_a);

    // ── Request 2: a second scope must be isolated ──
    let scope2 = provider.create_scope();
    let ctx2: Arc<DbContext> = scope2
        .get()
        .map_err(|e| Error::internal(format!("DbContext not registered: {e}")))?;
    check(
        "different scopes get different instances",
        !Arc::ptr_eq(&ctx1_a, &ctx2),
    )?;

    // ── Root provider: strict-scope validation ──
    let root_attempt = provider.get::<DbContext>();
    check(
        "root provider rejects a Scoped DbContext (request scope is mandatory)",
        root_attempt.is_err(),
    )?;

    // ── Owned resolution: the handler `#[inject(owned)]` path ──
    let owned1: DbContext = scope1
        .get_owned()
        .map_err(|e| Error::internal(format!("owned DbContext resolution failed: {e}")))?;
    let owned2: DbContext = scope1
        .get_owned()
        .map_err(|e| Error::internal(format!("owned DbContext resolution failed: {e}")))?;
    check("get_owned yields an owned DbContext", true)?;
    drop(owned1);
    drop(owned2);

    // ── Request end: dropping the scope releases what its cache held ──
    drop(ctx1_b);
    drop(scope1);
    drop(ctx2);
    drop(scope2);
    check(
        "dropping the scope releases its cached instance",
        Arc::strong_count(&ctx1_a) < refs_while_scoped,
    )?;

    // ── Request 3: a new scope still resolves, independently ──
    let scope3 = provider.create_scope();
    let ctx3: Arc<DbContext> = scope3
        .get()
        .map_err(|e| Error::internal(format!("DbContext not registered: {e}")))?;
    check(
        "a new scope gets a fresh instance",
        !Arc::ptr_eq(&ctx1_a, &ctx3),
    )?;
    drop(ctx3);
    drop(scope3);
    drop(ctx1_a);

    tracing::info!("[scope_smoke] all assertions passed");
    tracing::info!(
        "[scope_smoke] this is the per-request unit of work behind \
         `#[inject(owned)] ctx: DbContext` — no Arc<Mutex>, no ambient provider"
    );
    Ok(())
}
