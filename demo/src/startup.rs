//! Application startup — database initialization and seeding.
//!
//! Uses `AppDbContext` (not a global static) for database access.
//! Migrations are in `crate::migrations/`.

use std::sync::Arc;

use crate::domain::db_context::AppDbContext;

use lref::provider::DbValue;
use lref_provider_sqlite::SqliteProvider;

/// Initialize the database: create provider, run migrations, seed data.
///
/// Returns an `Arc<AppDbContext>` ready for DI registration.
pub async fn initialize() -> Arc<AppDbContext> {
    let sqlite = SqliteProvider::new("lrwf_demo.db").expect("Failed to open SQLite database");
    let provider = Arc::new(sqlite) as Arc<dyn lref::provider::DatabaseProvider>;
    let ctx = Arc::new(AppDbContext::new(Arc::clone(&provider)));

    // Run migrations
    crate::domain::migrations::m001_initial_20260611::up(&ctx)
        .await
        .expect("Migration failed");

    // Seed admin user
    if ctx
        .set::<crate::domain::user::UserEntity>()
        .filter_column("email", "=", DbValue::String("admin@lrwf.dev".into()))
        .first_or_default()
        .await
        .ok()
        .flatten()
        .is_none()
    {
        let hashed =
            bcrypt::hash("admin123", bcrypt::DEFAULT_COST).expect("Failed to hash admin password");
        let sql = format!(
            "INSERT INTO users (id, name, email, password_hash, role, created_at) \
             VALUES ('admin-001', 'Admin', 'admin@lrwf.dev', '{}', 'admin', '{}')",
            hashed.replace('\'', "''"),
            now_secs()
        );
        ctx.execute(&sql).await.expect("Failed to insert admin");
        tracing::info!("[DB] Default admin created: admin@lrwf.dev / admin123");
    }

    // Seed products
    use crate::domain::product::ProductEntity;
    let count = ctx.set::<ProductEntity>().count().await.unwrap_or(0);
    if count == 0 {
        for (name, price) in &[("Widget", 9.99), ("Gadget", 24.50), ("Thingamajig", 3.75)] {
            let sql = format!(
                "INSERT INTO products (id, name, price, created_at) VALUES ('{}', '{}', {}, '{}')",
                uuid(),
                name,
                price,
                now_secs()
            );
            ctx.execute(&sql).await.expect("Failed to seed product");
        }
        tracing::info!("[DB] Seeded 3 sample products");
    }

    tracing::info!("[DB] Initialization complete");
    ctx
}

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| format!("{:x}", d.as_nanos()))
        .unwrap_or_else(|_| "0".to_string())
}

fn now_secs() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}
