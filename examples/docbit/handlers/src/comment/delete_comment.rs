//! `DELETE /api/comments/{id}` — soft delete; author or admin only.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::comment::DeleteCommentRequest;
use docbit_domain::entities::Comment;

use crate::common::{now_secs, operator_id, parse_id, save_changes};

#[derive(Inject)]
pub struct DeleteCommentHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteCommentRequest, String> for DeleteCommentHandler {
    async fn handle(&mut self, req: DeleteCommentRequest) -> ActionResult<String> {
        let id = parse_id(&req.id)?;
        let q = id.clone();

        // ── 加载 ──
        let comment = linq!(self.ctx.set::<Comment>(), |c: Comment| c.id == q)
            .first_or_default()
            .await?
            .ok_or_else(|| Error::not_found("Comment not found"))?;

        // ── 归属：owner 取自行，不取 body；admin 可越权 ──
        let user = req.require_user()?;
        if !user.is_in_role("admin") && comment.user_id != user.id {
            return Err(Error::forbidden("can only delete your own comments"));
        }

        // ── 变更与持久化 ──
        self.ctx.edit(comment, |c| {
            c.is_deleted = true;
            c.updated_id = operator_id();
            c.updated_at = now_secs();
        });
        self.ctx.detect_changes();

        save_changes(&mut self.ctx).await?;

        tracing::info!("[Comment] Soft-deleted: {}", id);
        Ok(format!("Deleted comment {}", id))
    }
}
