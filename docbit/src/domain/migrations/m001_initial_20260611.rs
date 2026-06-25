//! Initial migration — creates core tables for portfolio site.

use rust_ef::db_context::{DbContext, IDbContext};

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
            "CREATE TABLE IF NOT EXISTS works (\
                id TEXT PRIMARY KEY NOT NULL, \
                slug TEXT NOT NULL, \
                title TEXT NOT NULL, \
                subtitle TEXT NOT NULL, \
                description TEXT NOT NULL, \
                category TEXT NOT NULL, \
                tags TEXT NOT NULL, \
                repo_url TEXT NOT NULL, \
                demo_url TEXT NOT NULL, \
                docs_slug TEXT NOT NULL, \
                featured INTEGER NOT NULL, \
                sort_order INTEGER NOT NULL, \
                created_at TEXT NOT NULL\
            ) STRICT",
        )
        .await
        .map_err(|e| e.to_string())?;

    ctx.provider()
        .execute_migration_command(
            "CREATE TABLE IF NOT EXISTS blog_posts (\
                id TEXT PRIMARY KEY NOT NULL, \
                slug TEXT NOT NULL, \
                title TEXT NOT NULL, \
                summary TEXT NOT NULL, \
                content TEXT NOT NULL, \
                tags TEXT NOT NULL, \
                published_at TEXT NOT NULL, \
                created_at TEXT NOT NULL\
            ) STRICT",
        )
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!("[Migration] 001_initial: tables created");
    Ok(())
}
