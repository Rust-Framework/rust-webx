//! Scoped DbContext registration with per-instance model preparation.

use std::sync::Arc;

use rust_dix::ServiceCollection;
use rust_ef::db_context::{DbContext, DbContextOptionsBuilder};

/// Extension for registering DbContext with a per-instance model hook.
pub trait EfServiceCollectionExt {
    /// Register scoped `DbContext`, invoking `prepare` on every new instance.
    ///
    /// Use `prepare` for global query filters. Seed data belongs in startup
    /// initialization only — not in this hook.
    fn add_ef_dbcontext(
        self,
        configure_options: impl FnOnce(&mut DbContextOptionsBuilder) + Send + Sync + 'static,
        prepare: impl Fn(&mut DbContext) + Send + Sync + 'static,
    ) -> Self;
}

impl EfServiceCollectionExt for ServiceCollection {
    fn add_ef_dbcontext(
        self,
        configure_options: impl FnOnce(&mut DbContextOptionsBuilder) + Send + Sync + 'static,
        prepare: impl Fn(&mut DbContext) + Send + Sync + 'static,
    ) -> Self {
        let mut builder = DbContextOptionsBuilder::new();
        configure_options(&mut builder);
        let options = Arc::new(builder.build());
        options
            .create_provider()
            .expect("DbContext provider initialization failed at startup");

        let prepare = Arc::new(prepare);
        self.scoped(move |_| {
            let mut ctx =
                DbContext::from_options(&options).expect("Failed to create DbContext");
            prepare(&mut ctx);
            Arc::new(ctx)
        })
    }
}
