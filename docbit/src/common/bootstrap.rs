//! Composition-root registrations for framework-adjacent infrastructure.
//!
//! Handler types and business services use `#[rust_dicore::inject_attr]` and are
//! collected by `ServiceCollection::from_injected()`. Only types that rust-dicore
//! cannot construct (DbContext options, `AppPaths`) are registered here.

use std::sync::Arc;

use rust_dicore::ServiceCollection;
use rust_ef::db_context::{DbContext, DbContextOptionsBuilder};
use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
use tokio::sync::Mutex;

use crate::common::paths::{manifest_dir, resolve_data_path};
use crate::common::AuditInterceptor;

/// Resolved application paths — shared by hosted services and business services.
#[derive(Clone)]
pub struct AppPaths {
    pub docs_root: std::path::PathBuf,
    pub blog_root: std::path::PathBuf,
    pub db_path: std::path::PathBuf,
    pub wwwroot: std::path::PathBuf,
}

impl AppPaths {
    pub fn resolve() -> Self {
        let manifest = manifest_dir();
        Self {
            wwwroot: manifest.join("wwwroot"),
            db_path: manifest.join("docbit.db"),
            docs_root: resolve_data_path("docs"),
            blog_root: resolve_data_path("blog-data"),
        }
    }
}

fn build_db_options(db_path: &std::path::Path) -> Arc<rust_ef::db_context::DbContextOptions> {
    let mut builder = DbContextOptionsBuilder::new();
    builder
        .use_sqlite(db_path.to_string_lossy().as_ref())
        .add_interceptor(AuditInterceptor);
    Arc::new(builder.build())
}

/// Register `AppPaths` and `DbContext`. Business services (`IDocumentService`,
/// `IBlogService`) and handlers register themselves via `inject_attr`.
pub fn configure(mut svc: ServiceCollection) -> ServiceCollection {
    let paths = AppPaths::resolve();

    tracing::info!("[docbit] docs root: {}", paths.docs_root.display());
    tracing::info!("[docbit] blog root: {}", paths.blog_root.display());
    tracing::info!("[docbit] database: {}", paths.db_path.display());

    let options = build_db_options(&paths.db_path);
    let paths_for_ctx = paths.clone();

    svc = svc.singleton::<AppPaths>(move |_| Arc::new(paths_for_ctx.clone()));

    svc = svc.singleton::<Mutex<DbContext>>(move |_| {
        let ctx = DbContext::from_options(&options).expect("Failed to create DbContext");
        Arc::new(Mutex::new(ctx))
    });

    svc
}
