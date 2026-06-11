use lrwf::*;

mod common;
mod contracts;
mod domain;
mod handlers;
mod startup;

#[tokio::main]
async fn main() {
    // 1. Initialize database — returns Arc<AppDbContext>
    let ctx = startup::initialize().await;

    // 2. Build host — register AppDbContext, handlers are auto-registered via #[inject_attr]
    let host = Host::builder()
        .mode(AppMode::Development)
        .use_spa("wwwroot")
        .use_auth()
        .register(move |svc| svc.instance(ctx))
        .use_memory_cache()
        .build();

    host.run().await.expect("Server failed");
}
