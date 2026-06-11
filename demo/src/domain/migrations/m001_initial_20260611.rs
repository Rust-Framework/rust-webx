//! Initial migration — creates the core database tables.
//!
//! Migration naming convention: `{seq}_{name}_{date}.rs`
//! Run from `startup::initialize()` after the provider is created.

use crate::domain::db_context::AppDbContext;

/// Run the initial migration: create `users` and `products` tables.
pub async fn up(ctx: &AppDbContext) -> Result<(), String> {
    ctx.execute(
        "CREATE TABLE IF NOT EXISTS users (\
            id TEXT PRIMARY KEY NOT NULL, \
            name TEXT NOT NULL, \
            email TEXT NOT NULL, \
            password_hash TEXT NOT NULL, \
            role TEXT NOT NULL, \
            created_at TEXT NOT NULL\
        ) STRICT",
    )
    .await?;

    ctx.execute(
        "CREATE TABLE IF NOT EXISTS products (\
            id TEXT PRIMARY KEY NOT NULL, \
            name TEXT NOT NULL, \
            price REAL NOT NULL, \
            created_at TEXT NOT NULL\
        ) STRICT",
    )
    .await?;

    tracing::info!("[Migration] 001_initial: tables created");
    Ok(())
}
