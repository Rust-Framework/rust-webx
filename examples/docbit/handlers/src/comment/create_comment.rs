//! `POST /api/comments` — any signed-in user may comment.

use rust_ef::db_context::DbContext;
use webx::*;

use docbit_contracts::comment::{CommentModel, CreateCommentRequest};
use docbit_domain::entities::Comment;

use crate::common::save_changes;

#[derive(Inject)]
pub struct CreateCommentHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateCommentRequest, CommentModel> for CreateCommentHandler {
    async fn handle(&mut self, req: CreateCommentRequest) -> ActionResult<CommentModel> {
        // ── 身份：作者只来自身份字段 ──
        let user = req.require_user()?;
        let user_id = user.id.clone();
        let user_name = if user.name.is_empty() {
            "User".to_string()
        } else {
            user.name.clone()
        };

        // ── 变更：内容在 TryFrom 中校验并归一化 ──
        let mut comment = Comment::try_from(req)?;
        comment.user_id = user_id;
        comment.user_name = user_name.clone();
        let model = CommentModel::from(&comment);

        let blog_id = comment.blog_id.clone();
        self.ctx.add(comment);

        save_changes(&mut self.ctx).await?;

        tracing::info!("[Comment] Created by {} on blog {}", user_name, blog_id);
        Ok(model)
    }
}
