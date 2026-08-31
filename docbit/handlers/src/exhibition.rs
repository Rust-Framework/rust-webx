//! Exhibition handlers — list / get / upsert / delete portfolio works.
//!
//! 每个 handler 持有 owned `DbContext`，`handle(&mut self, ...)` 直接操作 `self.ctx`。
//!
//! Public list/get overlay `repo_url` / `demo_url` from docs `INDEX.json` (via
//! `IDocumentService::get_portfolio`) so the site always reflects filesystem
//! metadata even when the DB row is stale (e.g. legacy gitcode URLs).

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use docbit_contracts::docs::IDocumentService;
use docbit_contracts::exhibition::{
    DeleteExhibitionRequest, ExhibitionModel, GetExhibitionRequest, ListExhibitionsRequest,
    UpsertExhibitionRequest,
};
use docbit_domain::entities::Exhibition;
use docbit_domain::{new_id, ApplyTo, ToEntity, ToModel};

use crate::db::{save_changes, EfResultExt};
use crate::util::{now_secs, operator_id};

/// Prefer INDEX.json `repoUrl` / `demoUrl` over DB values when present.
fn overlay_index_urls(docs: &dyn IDocumentService, model: &mut ExhibitionModel) {
    let key = model
        .docs_slug
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(model.slug.as_str());

    let Ok(from_index) = docs.get_portfolio(key) else {
        return;
    };

    if from_index.repo_url.is_some() {
        model.repo_url = from_index.repo_url;
    }
    if from_index.demo_url.is_some() {
        model.demo_url = from_index.demo_url;
    }
}

#[derive(Inject)]
pub struct ListExhibitionsHandler {
    #[inject(owned)]
    ctx: DbContext,
    #[inject]
    docs: Arc<dyn IDocumentService>,
}

#[derive(Inject)]
pub struct GetExhibitionHandler {
    #[inject(owned)]
    ctx: DbContext,
    #[inject]
    docs: Arc<dyn IDocumentService>,
}

#[derive(Inject)]
pub struct UpsertExhibitionHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct DeleteExhibitionHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListExhibitionsRequest, Vec<ExhibitionModel>> for ListExhibitionsHandler {
    async fn handle(&mut self, _: ListExhibitionsRequest) -> Result<Vec<ExhibitionModel>> {
        let items = linq!(self.ctx.set::<Exhibition>(); include e.category; order_by e.sort_order asc)
            .to_list()
            .await
            .map_ef()?;

        let mut models: Vec<ExhibitionModel> = items.into_iter().map(ExhibitionModel::from).collect();
        for model in &mut models {
            overlay_index_urls(self.docs.as_ref(), model);
        }
        Ok(models)
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetExhibitionRequest, ExhibitionModel> for GetExhibitionHandler {
    async fn handle(&mut self, req: GetExhibitionRequest) -> Result<ExhibitionModel> {
        let slug = req.slug.clone();
        let q = slug.clone();

        let item = linq!(self.ctx.set::<Exhibition>(), |e: Exhibition| e.slug == q; include e.category)
            .first_or_default()
            .await
            .map_ef()?
            .ok_or_else(|| Error::NotFound(format!("Exhibition not found: {}", slug)))?;

        let mut model = ExhibitionModel::from(item);
        overlay_index_urls(self.docs.as_ref(), &mut model);
        Ok(model)
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpsertExhibitionRequest, ExhibitionModel> for UpsertExhibitionHandler {
    async fn handle(&mut self, req: UpsertExhibitionRequest) -> Result<ExhibitionModel> {
        let now = now_secs();
        let slug = req.slug.clone();
        let q = slug.clone();

        let existing = linq!(self.ctx.set::<Exhibition>(), |e: Exhibition| e.slug == q)
            .first_or_default()
            .await
            .map_ef()?;

        let saved_id = if let Some(mut ex) = existing {
            let id = ex.id.clone();
            req.apply_to(&mut ex, now);

            let set = self.ctx.set::<Exhibition>();
            set.update(ex);

            save_changes(&mut self.ctx).await?;

            id
        } else {
            let id = new_id();
            let entity = req.to_entity(id.clone(), now);

            let set = self.ctx.set::<Exhibition>();
            set.add(entity);

            save_changes(&mut self.ctx).await?;

            id
        };

        let saved = crate::ef_require_by_id!(
            self.ctx,
            Exhibition,
            saved_id,
            Error::NotFound("Exhibition not found after save".into());
            include row.category
        );

        tracing::info!("[Exhibition] Upserted: {} ({})", saved.slug, saved.id);
        Ok(saved.to_model())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteExhibitionRequest, String> for DeleteExhibitionHandler {
    async fn handle(&mut self, req: DeleteExhibitionRequest) -> Result<String> {
        let now = now_secs();
        let slug = req.slug.clone();
        let q = slug.clone();

        let items = linq!(self.ctx.set::<Exhibition>(), |e: Exhibition| e.slug == q)
            .to_list()
            .await
            .map_ef()?;

        if items.is_empty() {
            return Err(Error::NotFound(format!("Exhibition not found: {}", slug)));
        }

        let set = self.ctx.set::<Exhibition>();
        for mut item in items {
            item.is_deleted = true;
            item.updated_id = operator_id();
            item.updated_at = now;
            set.update(item);
        }

        save_changes(&mut self.ctx).await?;

        tracing::info!("[Exhibition] Soft-deleted: {}", slug);
        Ok(format!("Deleted: {}", slug))
    }
}
