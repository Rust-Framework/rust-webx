//! Comment handlers — list / create (with claims) / delete (soft).
//!
//! Comment 的 `parent`/`quoted` 双自外键导航**不在 linq! 中 include**
//! （多 FK 导航绑定缺陷），列表仅返回扁平记录，前端按 id 二次拉取引用内容。
//!
//! 每个 handler 持有 owned `DbContext`，`handle(&mut self, ...)` 直接操作 `self.ctx`。

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use docbit_contracts::comment::{
    CommentModel, CreateCommentRequest, DeleteCommentRequest, ListCommentsRequest,
};
use docbit_domain::entities::Comment;
use docbit_domain::{new_id, ToEntity, ToModel};

use crate::db::{save_changes, EfResultExt};
use crate::util::{now_secs, operator_id, parse_id};

#[derive(Inject)]
pub struct ListCommentsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct CreateCommentHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct DeleteCommentHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListCommentsRequest, Vec<CommentModel>> for ListCommentsHandler {
    async fn handle(&mut self, req: ListCommentsRequest) -> Result<Vec<CommentModel>> {
        let blog_id = parse_id(&req.blog_id)?;
        let q = blog_id.clone();

        let mut rows = linq!(self.ctx.set::<Comment>(), |c: Comment| c.blog_id == q)
            .to_list()
            .await
            .map_ef()?;

        rows.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(rows.into_iter().map(CommentModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateCommentRequest, CommentModel> for CreateCommentHandler {
    async fn handle(&mut self, req: CreateCommentRequest) -> Result<CommentModel> {
        let claims = req
            .claims
            .as_deref()
            .ok_or_else(|| Error::Http("Not authenticated".into()))?;

        let content = req.content.trim().to_string();
        if content.is_empty() {
            return Err(Error::Http("Comment cannot be empty".into()));
        }
        if content.len() > 4000 {
            return Err(Error::Http("Comment too long".into()));
        }

        let user_id = claims.subject().to_string();
        let user_name = claims
            .get_username()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "User".to_string());

        let now = now_secs();
        let comment_id = new_id();

        let mut comment = req.to_entity(comment_id, now);
        comment.user_id = user_id;
        comment.user_name = user_name.clone();
        comment.content = content;

        let set = self.ctx.set::<Comment>();
        set.add(comment.clone());

        save_changes(&mut self.ctx).await?;

        tracing::info!("[Comment] Created by {} on blog {}", user_name, comment.blog_id);
        Ok(comment.to_model())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteCommentRequest, String> for DeleteCommentHandler {
    async fn handle(&mut self, req: DeleteCommentRequest) -> Result<String> {
        let id = parse_id(&req.id)?;

        let mut comment = self
            .ctx
            .set::<Comment>()
            .query()
            .find(id.clone())
            .await
            .map_ef()?
            .ok_or_else(|| Error::NotFound("Comment not found".into()))?;

        let claims = req
            .claims
            .as_deref()
            .ok_or_else(|| Error::Http("Not authenticated".into()))?;
        let uid = claims.subject().to_string();

        if !claims.has_role("admin") && comment.user_id != uid {
            return Err(Error::Http("Forbidden: can only delete your own comments".into()));
        }

        comment.is_deleted = true;
        comment.updated_id = operator_id();
        comment.updated_at = now_secs();

        let set = self.ctx.set::<Comment>();
        set.update(comment);

        save_changes(&mut self.ctx).await?;

        tracing::info!("[Comment] Soft-deleted: {}", id);
        Ok(format!("Deleted comment {}", id))
    }
}
