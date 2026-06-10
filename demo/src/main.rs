use lrwf::*;
use std::sync::Arc;

mod contracts;
mod domain;
mod handlers;

#[tokio::main]
async fn main() {
    // ── Dynamic authorization policy ──
    // Declares role/permission requirements per route pattern.
    // This complements static #[authorize] declarations on routes.
    // In production, swap ResourceAuthorization for a DB-backed policy.
    let authz = ResourceAuthorization::new()
        // Admin-only resources (matches #[authorize(role = "admin")] on routes)
        .allow_role("/api/users", "admin")
        .allow_role("/api/users/{id}", "admin")
        .allow_role("/api/products", "admin")
        .allow_role("/api/products/{id}", "admin")
        // Any authenticated user can access their profile
        .allow_role("/api/auth/me", "user")
        .allow_role("/api/auth/me", "admin");

    let host = Host::builder()
        .mode(AppMode::Development)
        .use_spa("wwwroot")
        // ── Authentication: JWT middleware, auto-config from appsettings ──
        .use_auth()
        // ── Dynamic authorization policy (checked per-request after routing) ──
        .use_authorization(Arc::new(authz))
        .configure(|app| {
            app.useOptions(|o| {
                o.app.name = format!("{} — LRWF Demo", o.app.name);
            });
        })
        .build();

    // 应用启动后创建默认管理员（仅首次运行）
    handlers::auth::ensure_admin_user();

    // Addresses read from appsettings.json → App.Urls. No manual passing needed.
    host.run().await.expect("Server failed");
}
