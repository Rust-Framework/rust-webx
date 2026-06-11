use lrwf::*;

mod common;
mod contracts;
mod domain;
mod handlers;

#[tokio::main]
async fn main() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .use_spa("wwwroot")
        .use_auth()
        .register(common::register_common_services)
        .use_memory_cache()
        .build();

    // Ensure database tables and seed data exist
    handlers::startup::initialize().await;

    // Read URLs from appsettings.json, auto-start
    host.run().await.expect("Server failed");
}
