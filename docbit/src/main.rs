use std::sync::Arc;

use rust_ef::db_context::{DbContext, DbContextOptionsBuilder};
use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
use rust_webapp::*;
use tokio::sync::Mutex;

mod common;
mod contracts;
mod domain;
mod handlers;
mod paths;
mod services;
mod startup;

use paths::{manifest_dir, resolve_data_path};
use services::docs::DocService;

#[tokio::main]
async fn main() {
    let db_path = manifest_dir().join("docbit.db");
    let docs_root = resolve_data_path("docs");

    tracing::info!("[docbit] docs root: {}", docs_root.display());
    tracing::info!("[docbit] database: {}", db_path.display());

    let mut opts_builder = DbContextOptionsBuilder::new();
    opts_builder
        .use_sqlite(db_path.to_string_lossy().as_ref())
        .add_interceptor(common::AuditInterceptor);

    let options = Arc::new(opts_builder.build());
    let docs = Arc::new(DocService::new(docs_root));

    let host = Host::builder()
        .mode(AppMode::Development)
        .register(move |svc| {
            let docs = Arc::clone(&docs);
            svc.singleton::<Mutex<DbContext>>(move |_resolver| {
                let ctx = DbContext::from_options(&options).expect("Failed to create DbContext");
                Arc::new(Mutex::new(ctx))
            })
            .singleton::<DocService>(move |_resolver| Arc::clone(&docs))
        })
        .use_spa(manifest_dir().join("wwwroot").to_string_lossy().into_owned())
        .use_auth()
        .use_memory_cache()
        .build();

    host.run().await.expect("Server failed");
}
