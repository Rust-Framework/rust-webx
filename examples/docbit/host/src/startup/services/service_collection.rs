//! `IServiceCollection` extensions — ASP.NET Core `AddDbContext` analogue.

use std::sync::Arc;

use docbit_domain::prepare_context;
use rust_ef::db_context::{DbContext, DbContextOptions, DbContextOptionsBuilder};
use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
use webx::rust_dix::ServiceCollection;
use webx::*;

/// Build and preflight the SQLite options once, at startup.
///
/// This is the only fallible step of the database registration, so it returns a
/// `Result` and the composition root propagates it:
///
/// * `create_provider()` opens/validates the database and **caches** the provider
///   on the options, so every later `DbContext::from_options` reuses it;
/// * one throwaway context pre-builds the process-wide metadata cache (entities,
///   indexes, query filters) and validates the model.
///
/// After that, the per-request factory has nothing left that can fail.
pub fn sqlite_db_options() -> Result<Arc<DbContextOptions>> {
    let path = app_base().join("app.db");
    let mut db = DbContextOptionsBuilder::new();
    db.use_sqlite(&path.to_string_lossy());
    tracing::info!("[docbit] SQLite path: {}", path.display());

    let options = Arc::new(db.build());
    options
        .create_provider()
        .map_err(|e| Error::di(format!("SQLite provider initialization failed: {}", e)))?;

    DbContext::from_options(&options)
        .map_err(|e| Error::di(format!("DbContext preflight failed: {}", e)))?;

    Ok(options)
}

/// Docbit DI registration helpers.
pub trait ServiceCollectionExt {
    /// scoped SQLite `DbContext` at `<app_base>/app.db`, built from preflighted
    /// options so the factory cannot fail per request.
    fn add_docbit_db(self, options: Arc<DbContextOptions>) -> Self;
}

impl ServiceCollectionExt for ServiceCollection {
    fn add_docbit_db(self, options: Arc<DbContextOptions>) -> Self {
        self.scoped(move |_| match DbContext::from_options(&options) {
            Ok(mut ctx) => {
                // Per-instance global filters: rust-ef keeps query filters on the
                // context's own model builder, so every new context re-registers
                // the soft-delete filters before any `set::<T>()`.
                prepare_context(&mut ctx);
                Arc::new(ctx)
            }
            Err(error) => {
                // Unreachable once `sqlite_db_options()` has cached the provider
                // and the metadata; a DI factory cannot return an error, so fail
                // closed instead of serving requests on a half-built context.
                tracing::error!(
                    "[docbit] DbContext creation failed after startup preflight: {error}"
                );
                std::process::abort();
            }
        })
    }
}
