//! Sync exhibition `repo_url` from docs INDEX.json (with GitHub fallbacks).
//!
//! EF `has_data` uses INSERT OR IGNORE / ON CONFLICT DO NOTHING, so existing
//! production rows keep stale or null `repo_url`. This pass updates (or inserts)
//! portfolio works on every startup. Prefer `meta.repoUrl` from INDEX.json via
//! `IDocumentService`; fall back to `canonical_exhibition_repo_urls` when a
//! work has no INDEX yet.

use std::collections::HashMap;

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use docbit_contracts::docs::IDocumentService;
use docbit_domain::entities::Exhibition;
use docbit_domain::seed::{canonical_exhibition_repo_urls, exhibition_seed_rows};
use docbit_handlers::db::{save_changes, EfResultExt};

/// Upsert `repo_url` for ecosystem exhibitions (by stable slug / seed id).
pub async fn ensure_exhibition_repo_urls(
    ctx: &mut DbContext,
    docs: &dyn IDocumentService,
) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut desired: HashMap<String, String> = HashMap::new();

    match docs.list_portfolio() {
        Ok(items) => {
            for item in items {
                if let Some(repo) = item.repo_url.filter(|u| !u.is_empty()) {
                    desired.insert(item.slug, repo);
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                "[DbInit] Could not read INDEX.json portfolio for repo_url sync: {}",
                e
            );
        }
    }

    for (slug, repo_url) in canonical_exhibition_repo_urls() {
        desired
            .entry((*slug).to_string())
            .or_insert_with(|| (*repo_url).to_string());
    }

    let mut updated = 0usize;
    let mut inserted = 0usize;

    for (slug, repo_url) in &desired {
        let q = slug.clone();
        let existing = linq!(ctx.set::<Exhibition>(), |e: Exhibition| e.slug == q)
            .first_or_default()
            .await
            .map_ef()?;

        if let Some(mut ex) = existing {
            let next = Some(repo_url.clone());
            if ex.repo_url != next {
                ex.repo_url = next;
                ex.updated_at = now;
                let set = ctx.set::<Exhibition>();
                set.update(ex);
                updated += 1;
            }
        } else if let Some(template) = exhibition_seed_rows()
            .iter()
            .find(|e| e.slug.as_str() == slug.as_str())
        {
            let mut row = (*template).clone();
            row.created_at = now;
            row.updated_at = now;
            row.repo_url = Some(repo_url.clone());
            let set = ctx.set::<Exhibition>();
            set.add(row);
            inserted += 1;
        }
    }

    if updated > 0 || inserted > 0 {
        save_changes(ctx).await?;
        tracing::info!(
            "[DbInit] Exhibition repo_url sync: updated={}, inserted={} (INDEX.json preferred)",
            updated,
            inserted
        );
    } else {
        tracing::info!("[DbInit] Exhibition repo_url sync: already up to date");
    }

    Ok(())
}
