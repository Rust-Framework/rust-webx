use rust_webapp::*;

mod common;
mod contracts;
mod domain;
mod handlers;
mod startup;

#[tokio::main]
async fn main() {
    let wwwroot = common::bootstrap::AppPaths::resolve().wwwroot;

    let host = Host::builder()
        .mode(AppMode::Development)
        .register(common::bootstrap::configure)
        .use_spa(wwwroot.to_string_lossy().into_owned())
        .use_auth()
        .use_memory_cache()
        .build();

    host.run().await.expect("Server failed");
}
