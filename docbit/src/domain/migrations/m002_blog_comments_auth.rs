//! Migration 002 — blog category, comments, password reset tokens.

use rust_ef::db_context::{DbContext, IDbContext};

pub async fn up(ctx: &mut DbContext) -> Result<(), String> {
    match ctx
        .provider()
        .execute_migration_command(
            "ALTER TABLE blog_posts ADD COLUMN category TEXT NOT NULL DEFAULT 'rust'",
        )
        .await
    {
        Ok(_) => {}
        Err(e) => {
            let msg = e.to_string();
            if !msg.contains("duplicate column") {
                match ctx
                    .provider()
                    .execute_migration_command(
                        "ALTER TABLE blog_posts ADD COLUMN category TEXT DEFAULT 'rust'",
                    )
                    .await
                {
                    Ok(_) => {}
                    Err(e2) => {
                        let m = e2.to_string();
                        if !m.contains("duplicate column") {
                            return Err(format!(
                                "Failed to add blog_posts.category: {} ({})",
                                e, e2
                            ));
                        }
                    }
                }
            }
        }
    }

    ctx.provider()
        .execute_migration_command(
            "UPDATE blog_posts SET category = 'rust' WHERE category IS NULL OR category = '' OR length(category) > 32",
        )
        .await
        .ok();

    ctx.provider()
        .execute_migration_command(
            "CREATE TABLE IF NOT EXISTS blog_comments (\
                id TEXT PRIMARY KEY NOT NULL, \
                post_slug TEXT NOT NULL, \
                user_id TEXT NOT NULL, \
                user_name TEXT NOT NULL, \
                content TEXT NOT NULL, \
                created_at TEXT NOT NULL\
            ) STRICT",
        )
        .await
        .map_err(|e| e.to_string())?;

    ctx.provider()
        .execute_migration_command(
            "CREATE TABLE IF NOT EXISTS password_reset_tokens (\
                token TEXT PRIMARY KEY NOT NULL, \
                user_id TEXT NOT NULL, \
                expires_at TEXT NOT NULL, \
                used INTEGER NOT NULL DEFAULT 0\
            ) STRICT",
        )
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!("[Migration] 002_blog_comments_auth: schema updated");
    Ok(())
}
