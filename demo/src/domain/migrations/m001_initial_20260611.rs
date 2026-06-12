//! Initial migration — creates the core database tables.
//!
//! Migration naming convention: `{seq}_{name}_{date}.rs`
//! Run from `DbInitService::start()` during host initialization.

use lref::db_context::{DbContext, IDbContext};

/// Run the initial migration: create `users` and `products` tables.
pub async fn up(ctx: &mut DbContext) -> Result<(), String> {
    ctx.provider()
        .execute_migration_command(
            "CREATE TABLE IF NOT EXISTS users (\
                id TEXT PRIMARY KEY NOT NULL, \
                name TEXT NOT NULL, \
                email TEXT NOT NULL, \
                password_hash TEXT NOT NULL, \
                role TEXT NOT NULL, \
                created_at TEXT NOT NULL\
            ) STRICT",
        )
        .await
        .map_err(|e| e.to_string())?;

    ctx.provider()
        .execute_migration_command(
            "CREATE TABLE IF NOT EXISTS products (\
                id TEXT PRIMARY KEY NOT NULL, \
                name TEXT NOT NULL, \
                price REAL NOT NULL, \
                created_at TEXT NOT NULL\
            ) STRICT",
        )
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!("[Migration] 001_initial: tables created");
    Ok(())
}
