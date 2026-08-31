//! Sync exhibition rows from docs INDEX.json into the DB (with GitHub fallbacks).
//!
//! EF `has_data` uses INSERT OR IGNORE / ON CONFLICT DO NOTHING, so existing
//! production rows keep stale metadata. This pass updates (or inserts) portfolio
//! works on every startup from INDEX.json via `IDocumentService`. Hardcoded
//! `canonical_exhibition_repo_urls` fill `repo_url` only when INDEX omits it.

use std::collections::HashSet;

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use docbit_contracts::docs::IDocumentService;
use docbit_contracts::exhibition::ExhibitionModel;
use docbit_domain::entities::Exhibition;
use docbit_domain::seed::{canonical_exhibition_repo_urls, exhibition_seed_rows};
use docbit_handlers::db::{save_changes, EfResultExt};

fn fallback_repo(slug: &str) -> Option<&'static str> {
    canonical_exhibition_repo_urls()
        .iter()
        .find(|(s, _)| *s == slug)
        .map(|(_, url)| *url)
}

fn resolved_repo(item: &ExhibitionModel) -> Option<String> {
    item.repo_url
        .clone()
        .filter(|u| !u.is_empty())
        .or_else(|| fallback_repo(&item.slug).map(str::to_string))
}

fn apply_index_fields(ex: &mut Exhibition, item: &ExhibitionModel, repo: Option<String>) -> bool {
    let mut dirty = false;

    if let Some(ref r) = repo {
        if ex.repo_url.as_deref() != Some(r.as_str()) {
            ex.repo_url = Some(r.clone());
            dirty = true;
        }
    }

    if let Some(ref d) = item.demo_url {
        if !d.is_empty() && ex.demo_url.as_deref() != Some(d.as_str()) {
            ex.demo_url = Some(d.clone());
            dirty = true;
        }
    }

    if !item.title.is_empty() && ex.title != item.title {
        ex.title = item.title.clone();
        dirty = true;
    }
    if !item.subtitle.is_empty() && ex.subtitle != item.subtitle {
        ex.subtitle = item.subtitle.clone();
        dirty = true;
    }
    if !item.description.is_empty() && ex.description != item.description {
        ex.description = item.description.clone();
        dirty = true;
    }

    if let Some(ref logo) = item.logo_url {
        if !logo.is_empty() && ex.logo_url.as_deref() != Some(logo.as_str()) {
            ex.logo_url = Some(logo.clone());
            dirty = true;
        }
    }

    if !item.tags.is_empty() {
        let tags_json = serde_json::to_string(&item.tags).unwrap_or_default();
        if ex.tags != tags_json {
            ex.tags = tags_json;
            dirty = true;
        }
    }

    if ex.featured != item.featured {
        ex.featured = item.featured;
        dirty = true;
    }
    if item.sort_order != 0 && ex.sort_order != item.sort_order {
        ex.sort_order = item.sort_order;
        dirty = true;
    }
    if item.docs_slug.is_some() && ex.docs_slug != item.docs_slug {
        ex.docs_slug = item.docs_slug.clone();
        dirty = true;
    }

    dirty
}

/// Upsert exhibition metadata from INDEX.json (by stable slug / seed id).
pub async fn ensure_exhibition_repo_urls(
    ctx: &mut DbContext,
    docs: &dyn IDocumentService,
) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let portfolio = match docs.list_portfolio() {
        Ok(items) => items,
        Err(e) => {
            tracing::warn!(
                "[DbInit] Could not read INDEX.json portfolio for exhibition sync: {}",
                e
            );
            Vec::new()
        }
    };

    let mut updated = 0usize;
    let mut inserted = 0usize;
    let mut seen: HashSet<String> = HashSet::new();

    for item in &portfolio {
        seen.insert(item.slug.clone());
        let repo = resolved_repo(item);
        let q = item.slug.clone();
        let existing = linq!(ctx.set::<Exhibition>(), |e: Exhibition| e.slug == q)
            .first_or_default()
            .await
            .map_ef()?;

        if let Some(mut ex) = existing {
            if apply_index_fields(&mut ex, item, repo) {
                ex.updated_at = now;
                let set = ctx.set::<Exhibition>();
                set.update(ex);
                updated += 1;
            }
        } else if let Some(template) = exhibition_seed_rows()
            .iter()
            .find(|e| e.slug.as_str() == item.slug.as_str())
        {
            let mut row = (*template).clone();
            row.created_at = now;
            row.updated_at = now;
            apply_index_fields(&mut row, item, repo);
            let set = ctx.set::<Exhibition>();
            set.add(row);
            inserted += 1;
        }
    }

    // Hardcoded GitHub fallback for known works missing from INDEX.
    for (slug, repo_url) in canonical_exhibition_repo_urls() {
        if seen.contains(*slug) {
            continue;
        }
        let q = (*slug).to_string();
        let existing = linq!(ctx.set::<Exhibition>(), |e: Exhibition| e.slug == q)
            .first_or_default()
            .await
            .map_ef()?;

        if let Some(mut ex) = existing {
            let next = Some((*repo_url).to_string());
            if ex.repo_url != next {
                ex.repo_url = next;
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
            "[DbInit] Exhibition INDEX sync: updated={}, inserted={} (DB is source for API)",
            updated,
            inserted
        );
    } else {
        tracing::info!("[DbInit] Exhibition INDEX sync: already up to date");
    }

    Ok(())
}
