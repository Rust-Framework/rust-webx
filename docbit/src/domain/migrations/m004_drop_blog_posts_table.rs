//! Drop legacy `blog_posts` table — posts now live under `blog-data/{user_id}/`.

use rust_ef::db_context::{DbContext, IDbContext};

pub async fn up(ctx: &mut DbContext) -> Result<(), String> {
    ctx.provider()
        .execute_migration_command("DROP TABLE IF EXISTS blog_posts")
        .await
        .map_err(|e| format!("Failed to drop blog_posts: {}", e))?;
    Ok(())
}
