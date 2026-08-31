//! Sync canonical exhibition GitHub `repo_url` values on every startup.
//!
//! EF `has_data` uses INSERT OR IGNORE / ON CONFLICT DO NOTHING, so existing
//! production rows keep stale or null `repo_url`. This pass updates (or inserts)
//! the five portfolio works so redeploys pick up GitHub links without wiping DB.

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use docbit_domain::entities::Exhibition;
use docbit_domain::seed::{canonical_exhibition_repo_urls, exhibition_seed_rows};
use docbit_handlers::db::{save_changes, EfResultExt};

/// Upsert `repo_url` for the five ecosystem exhibitions (by stable slug / seed id).
pub async fn ensure_exhibition_repo_urls(ctx: &mut DbContext) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut updated = 0usize;
    let mut inserted = 0usize;

    for (slug, repo_url) in canonical_exhibition_repo_urls() {
        let q = (*slug).to_string();
        let existing = linq!(ctx.set::<Exhibition>(), |e: Exhibition| e.slug == q)
            .first_or_default()
            .await
            .map_ef()?;

        if let Some(mut ex) = existing {
            let desired = Some((*repo_url).to_string());
            if ex.repo_url != desired {
                ex.repo_url = desired;
                ex.updated_at = now;
                let set = ctx.set::<Exhibition>();
                set.update(ex);
                updated += 1;
            }
        } else if let Some(template) = exhibition_seed_rows()
            .iter()
            .find(|e| e.slug.as_str() == *slug)
        {
            let mut row = (*template).clone();
            row.created_at = now;
            row.updated_at = now;
            row.repo_url = Some((*repo_url).to_string());
            let set = ctx.set::<Exhibition>();
            set.add(row);
            inserted += 1;
        }
    }

    if updated > 0 || inserted > 0 {
        save_changes(ctx).await?;
        tracing::info!(
            "[DbInit] Exhibition repo_url sync: updated={}, inserted={}",
            updated,
            inserted
        );
    } else {
        tracing::info!("[DbInit] Exhibition repo_url sync: already up to date");
    }

    Ok(())
}
