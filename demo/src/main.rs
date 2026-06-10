use lrwf::*;

mod contracts;
mod domain;
mod handlers;

#[tokio::main]
async fn main() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .use_spa("wwwroot")
        .build();

    // Addresses read from appsettings.json → App.Urls. No manual passing needed.
    host.run().await.expect("Server failed");
}
