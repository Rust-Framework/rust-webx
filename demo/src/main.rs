use std::sync::Arc;

use lref::db_context::{DbContext, DbContextOptionsBuilder};
use lref_provider_sqlite::DbContextOptionsBuilderExt as _;
use lrwf::*;
use tokio::sync::Mutex;

mod common;
mod contracts;
mod domain;
mod handlers;
mod startup;

#[tokio::main]
async fn main() {
    // Configure DbContext — EF Core 风格
    //   services.AddDbContext<AppDbContext>(o => {
    //       o.UseSqlite("...").AddInterceptor(new AuditInterceptor());
    //   })
    let mut opts_builder = DbContextOptionsBuilder::new();
    opts_builder
        .use_sqlite("lrwf_demo.db")
        .add_interceptor(common::AuditInterceptor);

    let options = Arc::new(opts_builder.build());

    let host = Host::builder()
        .mode(AppMode::Development)
        .register(move |svc| {
            svc.singleton::<Mutex<DbContext>>(move |_resolver| {
                let ctx = DbContext::from_options(&options).expect("Failed to create DbContext");
                Arc::new(Mutex::new(ctx))
            })
        })
        .use_spa("wwwroot")
        .use_auth()
        .use_memory_cache()
        .build();

    host.run().await.expect("Server failed");
}
