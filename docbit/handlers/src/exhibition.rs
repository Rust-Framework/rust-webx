//! Exhibition handlers — list / get / upsert portfolio works.
//!
//! 每个 handler 持有 owned `DbContext`，`handle(&mut self, ...)` 直接操作 `self.ctx`。

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use docbit_contracts::exhibition::{
    DeleteExhibitionRequest, ExhibitionModel, GetExhibitionRequest, ListExhibitionsRequest,
    UpsertExhibitionRequest,
};
use docbit_domain::entities::Exhibition;
use docbit_domain::{new_id, ApplyTo, ToEntity, ToModel};

use crate::util::{now_secs, operator_id};

#[derive(Inject)]
pub struct ListExhibitionsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct GetExhibitionHandler {
    #[inject(owned)]
    ctx: DbContext,
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
        let items = linq!(self.ctx.set::<Exhibition>(), |e: Exhibition| !e.is_deleted; include e.category; order_by e.sort_order asc)
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(items.into_iter().map(ExhibitionModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetExhibitionRequest, ExhibitionModel> for GetExhibitionHandler {
    async fn handle(&mut self, req: GetExhibitionRequest) -> Result<ExhibitionModel> {
        let slug = req.slug.clone();
        let item = linq!(self.ctx.set::<Exhibition>(), |e: Exhibition| e.slug == req.slug && !e.is_deleted; include e.category)
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound(format!("Exhibition not found: {}", slug)))?;
        Ok(ExhibitionModel::from(item))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpsertExhibitionRequest, ExhibitionModel> for UpsertExhibitionHandler {
    async fn handle(&mut self, req: UpsertExhibitionRequest) -> Result<ExhibitionModel> {
        let op = operator_id(req.claims.as_deref());
        let now = now_secs();

        // 按 slug 查找是否已存在（未软删除）
        let slug = req.slug.clone();
        let existing = {
            let q = slug.clone();
            linq!(self.ctx.set::<Exhibition>(), |e: Exhibition| e.slug == q && !e.is_deleted)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };

        let saved_id = if let Some(mut ex) = existing {
            let id = ex.id.clone();
            req.apply_to(&mut ex, op.clone(), now);
            let set = self.ctx.set::<Exhibition>();
            set.update(ex);
            self.ctx
                .save_changes()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
            id
        } else {
            let id = new_id();
            let entity = req.to_entity(id.clone(), op.clone(), now);
            let set = self.ctx.set::<Exhibition>();
            set.add(entity);
            self.ctx
                .save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to create exhibition: {}", e)))?;
            id
        };

        let saved = linq!(self.ctx.set::<Exhibition>(), |e: Exhibition| e.id == saved_id && !e.is_deleted; include e.category)
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::Internal("Exhibition vanished after save".into()))?;
        tracing::info!("[Exhibition] Upserted: {} ({})", saved.slug, saved.id);
        Ok(saved.to_model())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteExhibitionRequest, String> for DeleteExhibitionHandler {
    async fn handle(&mut self, req: DeleteExhibitionRequest) -> Result<String> {
        let op = operator_id(req.claims.as_deref());
        let now = now_secs();

        let slug = req.slug.clone();
        let q = slug.clone();
        let items = linq!(self.ctx.set::<Exhibition>(), |e: Exhibition| e.slug == q && !e.is_deleted)
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        if items.is_empty() {
            return Err(Error::NotFound(format!("Exhibition not found: {}", slug)));
        }

        let set = self.ctx.set::<Exhibition>();
        for mut item in items {
            item.is_deleted = true;
            item.updated_id = op.clone();
            item.updated_at = now;
            set.update(item);
        }
        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        tracing::info!("[Exhibition] Soft-deleted: {}", slug);
        Ok(format!("Deleted: {}", slug))
    }
}
