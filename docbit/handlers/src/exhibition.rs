//! Exhibition handlers — list / get / upsert portfolio works.

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webapp::*;
use tokio::sync::Mutex;

use docbit_contracts::exhibition::{
    DeleteExhibitionRequest, ExhibitionModel, GetExhibitionRequest, ListExhibitionsRequest,
    UpsertExhibitionRequest,
};
use docbit_domain::entities::Exhibition;
use docbit_domain::{ApplyTo, ToEntity, ToModel};

use crate::util::{now_secs, operator_id};

#[derive(Inject)]
pub struct ListExhibitionsHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct GetExhibitionHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct UpsertExhibitionHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct DeleteExhibitionHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[inject(scoped)]
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

#[inject(scoped)]
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

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<UpsertExhibitionRequest, ExhibitionModel> for UpsertExhibitionHandler {
    async fn handle(&self, req: UpsertExhibitionRequest) -> Result<ExhibitionModel> {
        let op = operator_id(req.claims.as_deref());
        let now = now_secs();

        // 按 slug 查找是否已存在（未软删除）
        let slug = req.slug.clone();
        let existing = {
            let mut ctx = self.ctx.lock().await;
            let q = slug.clone();
            linq!(ctx.set::<Exhibition>(), |e: Exhibition| e.slug == q && !e.is_deleted)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };

        if let Some(mut ex) = existing {
            req.apply_to(&mut ex, op.unwrap_or(0), now);
            {
                let mut ctx = self.ctx.lock().await;
                ctx.set::<Exhibition>().update(ex);
                ctx.save_changes()
                    .await
                    .map_err(|e| Error::Internal(e.to_string()))?;
            }
        } else {
            let ex = req.to_entity(op.unwrap_or(0), now);
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
            let q = slug.clone();
            linq!(ctx.set::<Exhibition>(), |e: Exhibition| e.slug == q && !e.is_deleted; include e.category)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::Internal("Exhibition vanished after save".into()))?;
        tracing::info!("[Exhibition] Upserted: {} ({})", saved.slug, saved.id);
        Ok(saved.to_model())
    }
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<DeleteExhibitionRequest, String> for DeleteExhibitionHandler {
    async fn handle(&self, req: DeleteExhibitionRequest) -> Result<String> {
        let op = operator_id(req.claims.as_deref());
        let now = now_secs();

        let slug = req.slug.clone();
        let mut ctx = self.ctx.lock().await;
        let q = slug.clone();
        let items = linq!(ctx.set::<Exhibition>(), |e: Exhibition| e.slug == q && !e.is_deleted)
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        if items.is_empty() {
            return Err(Error::NotFound(format!("Exhibition not found: {}", slug)));
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

        tracing::info!("[Exhibition] Soft-deleted: {}", slug);
        Ok(format!("Deleted: {}", slug))
    }
}
