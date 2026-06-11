use lrwf::*;
use std::sync::Arc;

mod contracts;
mod domain;
mod handlers;

#[tokio::main]
async fn main() {
    let authz = ResourceAuthorization::new()
        .allow_role("/api/users", "admin")
        .allow_role("/api/users/{id}", "admin")
        .allow_role("/api/products", "admin")
        .allow_role("/api/products/{id}", "admin")
        .allow_role("/api/auth/me", "user")
        .allow_role("/api/auth/me", "admin");

    let host = Host::builder()
        .mode(AppMode::Development)
        .use_spa("wwwroot")
        .use_auth()
        .use_authorization(Arc::new(authz))
        // ── Cache: in-process memory cache with 10000 entry limit ──
        .use_memory_cache()
        .configure(|app| {
            app.useOptions(|o| {
                o.app.name = format!("{} — LRWF Demo", o.app.name);
            });
        })
        .build();

    // Ensure default admin user exists
    handlers::auth::ensure_admin_user();

    // Read URLs from appsettings.json, auto-start
    host.run().await.expect("Server failed");
}
