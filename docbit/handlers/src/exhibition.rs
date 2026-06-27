//! Exhibition handlers — list / get / upsert portfolio works.

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*, provider::DbValue};
use rust_webapp::*;
use tokio::sync::Mutex;

use docbit_contracts::exhibition::{
    DeleteExhibitionRequest, ExhibitionModel, GetExhibitionRequest, ListExhibitionsRequest,
    UpsertExhibitionRequest,
};
use docbit_domain::entities::Exhibition;

use crate::util::{now_secs, operator_id};

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListExhibitionsRequest, Vec<ExhibitionModel>>)]
pub struct ListExhibitionsHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<GetExhibitionRequest, ExhibitionModel>)]
pub struct GetExhibitionHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<UpsertExhibitionRequest, ExhibitionModel>)]
pub struct UpsertExhibitionHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<DeleteExhibitionRequest, String>)]
pub struct DeleteExhibitionHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListExhibitionsRequest, Vec<ExhibitionModel>> for ListExhibitionsHandler {
    async fn handle(&self, _: ListExhibitionsRequest) -> Result<Vec<ExhibitionModel>> {
        let items = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Exhibition>(), |e: Exhibition| !e.is_deleted; include e.category; order_by e.sort_order asc)
                .to_list()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        Ok(items.into_iter().map(ExhibitionModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetExhibitionRequest, ExhibitionModel> for GetExhibitionHandler {
    async fn handle(&self, req: GetExhibitionRequest) -> Result<ExhibitionModel> {
        let slug = req.slug.clone();
        let item = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Exhibition>(), |e: Exhibition| e.slug == req.slug && !e.is_deleted; include e.category)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound(format!("Exhibition not found: {}", slug)))?;
        Ok(ExhibitionModel::from(item))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpsertExhibitionRequest, ExhibitionModel> for UpsertExhibitionHandler {
    async fn handle(&self, _: UpsertExhibitionRequest) -> Result<ExhibitionModel> {
        unreachable!("handle_with_claims is always called")
    }
    async fn handle_with_claims(
        &self,
        req: UpsertExhibitionRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<ExhibitionModel> {
        let op = operator_id(claims);
        let now = now_secs();
        let tags_json = serde_json::to_string(&req.tags)
            .map_err(|e| Error::Internal(format!("tags serialize: {}", e)))?;

        // 按 slug 查找是否已存在（未软删除）
        let existing = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Exhibition>()
                .query()
                .filter_column("slug", "=", DbValue::String(req.slug.clone()))
                .filter_column("is_deleted", "=", DbValue::Bool(false))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };

        if let Some(mut ex) = existing {
            ex.title = req.title;
            ex.subtitle = req.subtitle;
            ex.description = req.description;
            ex.category_id = req.category_id;
            ex.tags = tags_json;
            ex.repo_url = req.repo_url;
            ex.demo_url = req.demo_url;
            ex.docs_slug = req.docs_slug;
            ex.featured = req.featured;
            ex.sort_order = req.sort_order;
            ex.logo_url = req.logo_url;
            ex.updated_id = op;
            ex.updated_at = now;
            {
                let mut ctx = self.ctx.lock().await;
                ctx.set::<Exhibition>().update(ex);
                ctx.save_changes()
                    .await
                    .map_err(|e| Error::Internal(e.to_string()))?;
            }
        } else {
            let ex = Exhibition {
                id: 0,
                slug: req.slug.clone(),
                title: req.title,
                subtitle: req.subtitle,
                description: req.description,
                category_id: req.category_id,
                tags: tags_json,
                repo_url: req.repo_url,
                demo_url: req.demo_url,
                docs_slug: req.docs_slug,
                featured: req.featured,
                sort_order: req.sort_order,
                logo_url: req.logo_url,
                created_at: now,
                updated_at: now,
                created_id: op,
                updated_id: op,
                is_deleted: false,
                category: BelongsTo::new(),
            };
            {
                let mut ctx = self.ctx.lock().await;
                ctx.set::<Exhibition>().add(ex);
                ctx.save_changes()
                    .await
                    .map_err(|e| Error::Internal(format!("Failed to create exhibition: {}", e)))?;
            }
        }

        let saved = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Exhibition>(), |e: Exhibition| e.slug == req.slug && !e.is_deleted; include e.category)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::Internal("Exhibition vanished after save".into()))?;
        tracing::info!("[Exhibition] Upserted: {} ({})", saved.slug, saved.id);
        Ok(ExhibitionModel::from(saved))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteExhibitionRequest, String> for DeleteExhibitionHandler {
    async fn handle_with_claims(
        &self,
        req: DeleteExhibitionRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<String> {
        let op = operator_id(claims);
        let now = now_secs();

        let mut ctx = self.ctx.lock().await;
        let items = ctx
            .set::<Exhibition>()
            .query()
            .filter_column("slug", "=", DbValue::String(req.slug.clone()))
            .filter_column("is_deleted", "=", DbValue::Bool(false))
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        if items.is_empty() {
            return Err(Error::NotFound(format!("Exhibition not found: {}", req.slug)));
        }

        for mut item in items {
            item.is_deleted = true;
            item.updated_id = op;
            item.updated_at = now;
            ctx.set::<Exhibition>().update(item);
        }
        ctx.save_changes()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        tracing::info!("[Exhibition] Soft-deleted: {}", req.slug);
        Ok(format!("Deleted: {}", req.slug))
    }

    async fn handle(&self, _: DeleteExhibitionRequest) -> Result<String> {
        unreachable!("handle_with_claims is always called")
    }
}
