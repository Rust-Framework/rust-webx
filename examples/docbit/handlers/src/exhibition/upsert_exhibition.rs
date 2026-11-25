//! `POST /api/exhibitions` — create or update one work's metadata (admin).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::exhibition::{ExhibitionModel, UpsertExhibitionRequest};
use docbit_domain::entities::Exhibition;

use crate::common::save_changes;

#[derive(Inject)]
pub struct UpsertExhibitionHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpsertExhibitionRequest, ExhibitionModel> for UpsertExhibitionHandler {
    async fn handle(&mut self, req: UpsertExhibitionRequest) -> ActionResult<ExhibitionModel> {
        let q = req.slug.clone();

        // ── 加载：slug 已存在则更新，否则插入 ──
        let existing = linq!(self.ctx.set::<Exhibition>(), |e: Exhibition| e.slug == q)
            .first_or_default()
            .await?;

        let saved_id = match existing {
            Some(ex) => {
                let id = ex.id.clone();
                self.ctx.edit(ex, |e| e.patch(req));
                id
            }
            None => {
                let entity = Exhibition::try_from(req)?;
                let id = entity.id.clone();
                self.ctx.add(entity);
                id
            }
        };
        self.ctx.detect_changes();

        save_changes(&mut self.ctx).await?;

        // ── 响应：带分类导航重新加载 ──
        let saved = crate::ef_require_by_id!(
            self.ctx,
            Exhibition,
            saved_id,
            Error::not_found("Exhibition not found after save");
            include row.category
        );

        tracing::info!("[Exhibition] Upserted: {} ({})", saved.slug, saved.id);
        Ok(ExhibitionModel::from(&saved))
    }
}
