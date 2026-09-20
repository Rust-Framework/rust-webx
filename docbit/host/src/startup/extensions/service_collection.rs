//! `IServiceCollection` extensions — ASP.NET Core `AddDbContext` analogue.

use std::sync::Arc;

use docbit_domain::prepare_context;
use rust_ef::db_context::{DbContext, DbContextOptionsBuilder};
use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
use webx::rust_dix::ServiceCollection;
use webx::*;

/// Docbit DI registration helpers.
pub trait ServiceCollectionExt {
    /// Scoped SQLite `DbContext` at `<app_base>/app.db`.
    fn add_docbit_db(self) -> Self;
}

impl ServiceCollectionExt for ServiceCollection {
    fn add_docbit_db(self) -> Self {
        let path = app_base().join("app.db");
        let mut db = DbContextOptionsBuilder::new();
        db.use_sqlite(&path.to_string_lossy());
        tracing::info!("[docbit] SQLite path: {}", path.display());

        let options = Arc::new(db.build());
        options
            .create_provider()
            .expect("DbContext provider initialization failed at startup");

        self.scoped(move |_| {
            let mut ctx = DbContext::from_options(&options).expect("Failed to create DbContext");
            prepare_context(&mut ctx);
            Arc::new(ctx)
        })
    }
}
