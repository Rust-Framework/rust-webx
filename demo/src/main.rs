use lrwf::*;

mod contracts;
mod domain;
mod handlers;

#[tokio::main]
async fn main() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .configure(|app| {
            app.useOptions(|o| {
                // Settings loaded from appsettings.json — override anything here.
                o.app.name = format!("{} — LRWF Demo", o.app.name);
                // CORS auto-configured from Cors section. Customize if needed:
                // o.cors.origins = vec!["https://myapp.com".into()];
            });
        })
        .build();

    // Addresses read from appsettings.json → App.Urls. No manual passing needed.
    host.run().await.expect("Server failed");
}
