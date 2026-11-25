//! `DELETE /api/exhibitions/{slug}` — soft delete (admin).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::exhibition::DeleteExhibitionRequest;
use docbit_domain::entities::Exhibition;

use crate::common::{now_secs, operator_id, save_changes, MAX_LIST_ROWS};

#[derive(Inject)]
pub struct DeleteExhibitionHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteExhibitionRequest, String> for DeleteExhibitionHandler {
    async fn handle(&mut self, req: DeleteExhibitionRequest) -> ActionResult<String> {
        let now = now_secs();
        let q = req.slug.clone();

        // ── 加载：slug 理论上唯一，历史数据可能有多行，一并软删 ──
        let items = linq!(self.ctx.set::<Exhibition>(), |e: Exhibition| e.slug == q)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;

        if items.is_empty() {
            return Err(Error::not_found(format!(
                "Exhibition not found: {}",
                req.slug
            )));
        }

        // ── 变更与持久化 ──
        for item in items {
            self.ctx.edit(item, |e| {
                e.is_deleted = true;
                e.updated_id = operator_id();
                e.updated_at = now;
            });
        }
        self.ctx.detect_changes();

        save_changes(&mut self.ctx).await?;

        tracing::info!("[Exhibition] Soft-deleted: {}", req.slug);
        Ok(format!("Deleted: {}", req.slug))
    }
}
