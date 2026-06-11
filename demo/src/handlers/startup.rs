use lref::provider::{DatabaseProvider, DbValue};
use lref::query::QueryBuilder;
use lref_provider_sqlite::SqliteProvider;
use std::sync::{Arc, OnceLock};

use crate::domain::product::ProductEntity;
use crate::domain::user::UserEntity;

// ── Global provider ──

static PROVIDER: OnceLock<Arc<SqliteProvider>> = OnceLock::new();

pub fn provider() -> Arc<SqliteProvider> {
    Arc::clone(PROVIDER.get().expect("DB not initialized"))
}

pub fn provider_dyn() -> Arc<dyn DatabaseProvider> {
    provider() as Arc<dyn DatabaseProvider>
}

/// Helper: execute raw SQL (for INSERT/UPDATE/DELETE where QueryBuilder is insufficient).
pub async fn exec(sql: &str) -> Result<(), String> {
    provider()
        .execute_migration_command(sql)
        .await
        .map_err(|e| e.to_string())
}

/// Initialize the database.
pub async fn initialize() {
    let sqlite = SqliteProvider::new("lrwf_demo.db").expect("Failed to open SQLite database");

    // Create tables
    sqlite
        .execute_migration_command(
            "CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, email TEXT NOT NULL, password_hash TEXT NOT NULL, role TEXT NOT NULL, created_at TEXT NOT NULL) STRICT",
        )
        .await
        .expect("Failed to create users table");

    sqlite
        .execute_migration_command(
            "CREATE TABLE IF NOT EXISTS products (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, price REAL NOT NULL, created_at TEXT NOT NULL) STRICT",
        )
        .await
        .expect("Failed to create products table");

    let provider = Arc::new(sqlite);

    // Seed admin user
    let qb = QueryBuilder::<UserEntity>::with_provider("users", Arc::clone(&provider) as Arc<dyn DatabaseProvider>);
    match qb
        .filter_column("email", "=", DbValue::String("admin@lrwf.dev".into()))
        .first_or_default()
        .await
    {
        Ok(Some(_)) => tracing::info!("[DB] Admin user already exists"),
        Ok(None) => {
            let hashed =
                bcrypt::hash("admin123", bcrypt::DEFAULT_COST).expect("Failed to hash admin password");
            let sql = format!(
                "INSERT INTO users (id, name, email, password_hash, role, created_at) VALUES ('{}', 'Admin', 'admin@lrwf.dev', '{}', 'admin', '{}')",
                "admin@lrwf.dev", hashed.replace('\'', "''"), now_secs()
            );
            provider
                .execute_migration_command(&sql)
                .await
                .expect("Failed to insert admin");
            tracing::info!("[DB] Default admin created: admin@lrwf.dev / admin123");
        }
        Err(e) => tracing::warn!("[DB] Error checking admin: {}", e),
    }

    // Seed products
    let pqb =
        QueryBuilder::<ProductEntity>::with_provider("products", Arc::clone(&provider) as Arc<dyn DatabaseProvider>);
    let count = pqb.count().await.unwrap_or(0);
    if count == 0 {
        for (name, price) in &[("Widget", 9.99), ("Gadget", 24.50), ("Thingamajig", 3.75)] {
            let sql = format!(
                "INSERT INTO products (id, name, price, created_at) VALUES ('{}', '{}', {}, '{}')",
                uuid(),
                name,
                price,
                now_secs()
            );
            provider
                .execute_migration_command(&sql)
                .await
                .expect("Failed to seed product");
        }
        tracing::info!("[DB] Seeded 3 sample products");
    }

    PROVIDER.set(provider).expect("DB already initialized");
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
