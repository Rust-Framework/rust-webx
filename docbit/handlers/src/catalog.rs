//! Catalog synchronisation — `docs/*/INDEX.json` → exhibition rows → `wwwroot` logos.
//!
//! This lives in the handlers layer, not in host startup, so it can run again at
//! runtime: after an operator uploads a documentation bundle the catalog is
//! re-synced in place, with no restart.
//!
//! EF `has_data` is `INSERT OR IGNORE`, so rows written by an earlier deploy keep
//! stale metadata. This pass updates (or inserts) every work from its INDEX.json.

use std::collections::HashSet;
use std::path::Path;

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::docs::{IDocumentService, ResyncSummary};
use docbit_contracts::exhibition::ExhibitionModel;
use docbit_domain::entities::Exhibition;
use docbit_domain::new_id;
use docbit_domain::seed::{canonical_exhibition_repo_urls, category_id_for, exhibition_seed_rows};

use crate::db::{save_changes, EfResultExt};

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

/// Copy INDEX.json metadata onto an existing row. Returns whether anything moved.
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

/// Build a row for a work that has no seed template — anything an operator
/// uploaded. Without this, uploading docs for a new work would serve the
/// documentation but never list the work.
fn row_from_model(item: &ExhibitionModel, repo: Option<String>, now: i64) -> Exhibition {
    Exhibition {
        id: new_id(),
        slug: item.slug.clone(),
        title: item.title.clone(),
        subtitle: item.subtitle.clone(),
        description: item.description.clone(),
        category_id: category_id_for(&item.category).to_string(),
        tags: serde_json::to_string(&item.tags).unwrap_or_else(|_| "[]".into()),
        repo_url: repo,
        demo_url: item.demo_url.clone().filter(|d| !d.is_empty()),
        docs_slug: item.docs_slug.clone(),
        featured: item.featured,
        sort_order: item.sort_order,
        logo_url: item.logo_url.clone().filter(|l| !l.is_empty()),
        created_at: now,
        updated_at: now,
        created_id: None,
        updated_id: None,
        is_deleted: false,
        category: Default::default(),
    }
}

/// Upsert exhibition metadata from every work's INDEX.json (by stable slug).
pub async fn sync_exhibitions(
    ctx: &mut DbContext,
    docs: &dyn IDocumentService,
) -> Result<ResyncSummary> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let portfolio = match docs.list_portfolio() {
        Ok(items) => items,
        Err(e) => {
            tracing::warn!("[Catalog] Could not read INDEX.json portfolio: {}", e);
            Vec::new()
        }
    };

    let mut summary = ResyncSummary {
        works: portfolio.len(),
        ..Default::default()
    };
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
                ctx.update(ex);
                summary.updated += 1;
            }
        } else if let Some(template) = exhibition_seed_rows()
            .iter()
            .find(|e| e.slug.as_str() == item.slug.as_str())
        {
            let mut row = (*template).clone();
            row.created_at = now;
            row.updated_at = now;
            apply_index_fields(&mut row, item, repo);
            ctx.add(row);
            summary.inserted += 1;
        } else {
            // Unknown work (typically freshly uploaded): create it from INDEX.json.
            ctx.add(row_from_model(item, repo, now));
            summary.inserted += 1;
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
                ctx.update(ex);
                summary.updated += 1;
            }
        } else if let Some(template) = exhibition_seed_rows()
            .iter()
            .find(|e| e.slug.as_str() == *slug)
        {
            let mut row = (*template).clone();
            row.created_at = now;
            row.updated_at = now;
            row.repo_url = Some((*repo_url).to_string());
            ctx.add(row);
            summary.inserted += 1;
        }
    }

    if summary.updated > 0 || summary.inserted > 0 {
        save_changes(ctx).await?;
    }

    Ok(summary)
}

/// Full catalog refresh: indexes → exhibition rows → logo copies.
///
/// Safe to call at boot and after an upload. `wwwroot` is the SPA overlay
/// directory whose `assets/works/` receives the logos.
pub async fn resync_catalog(
    ctx: &mut DbContext,
    docs: &dyn IDocumentService,
    wwwroot: &Path,
) -> Result<ResyncSummary> {
    docs.ensure_all_indexes()
        .map_err(|e| Error::Internal(format!("Doc index generation failed: {}", e)))?;

    let mut summary = sync_exhibitions(ctx, docs).await?;

    summary.logos = sync_logos(docs, wwwroot)?;

    tracing::info!(
        "[Catalog] Resync: works={}, updated={}, inserted={}, logos={}",
        summary.works,
        summary.updated,
        summary.inserted,
        summary.logos
    );
    Ok(summary)
}

/// Copy each work's logo into `wwwroot/assets/works/{slug}.{ext}`.
///
/// The service owns the copy; the count is read back so the caller can log how
/// many logos the destination holds after the pass.
fn sync_logos(docs: &dyn IDocumentService, wwwroot: &Path) -> Result<usize> {
    docs.sync_portfolio_assets(wwwroot)
        .map_err(|e| Error::Internal(format!("Portfolio asset sync failed: {}", e)))?;

    let count = std::fs::read_dir(wwwroot.join("assets/works"))
        .map(|entries| entries.flatten().filter(|e| e.path().is_file()).count())
        .unwrap_or(0);
    Ok(count)
}
