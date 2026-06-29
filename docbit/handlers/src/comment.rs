//! Comment handlers — list / create (with claims) / delete (soft or hard).
//!
//! Comment 的 `parent`/`quoted` 双自外键导航**不在 linq! 中 include**
//! （多 FK 导航绑定缺陷），列表仅返回扁平记录，前端按 id 二次拉取引用内容。

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webapp::*;
use tokio::sync::Mutex;

use docbit_contracts::comment::{
    CommentModel, CreateCommentRequest, DeleteCommentRequest, ListCommentsRequest,
};
use docbit_domain::entities::Comment;
use docbit_domain::{ToEntity, ToModel};

use crate::util::{now_secs, operator_id, parse_id};

#[derive(Inject)]
pub struct ListCommentsHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct CreateCommentHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct DeleteCommentHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<ListCommentsRequest, Vec<CommentModel>> for ListCommentsHandler {
    async fn handle(&self, req: ListCommentsRequest) -> Result<Vec<CommentModel>> {
        let blog_id = parse_id(&req.blog_id)?;
        let mut rows = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Comment>(), |c: Comment| c.blog_id == blog_id && !c.is_deleted)
                .to_list()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        rows.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(rows.into_iter().map(CommentModel::from).collect())
    }
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<CreateCommentRequest, CommentModel> for CreateCommentHandler {
    async fn handle(&self, req: CreateCommentRequest) -> Result<CommentModel> {
        let claims_ref = req.claims.as_deref();
        let claims = claims_ref.ok_or_else(|| Error::Http("Not authenticated".into()))?;
        let content_owned: String = req.content.trim().to_string();
        if content_owned.is_empty() {
            return Err(Error::Http("Comment cannot be empty".into()));
        }
        if content_owned.len() > 4000 {
            return Err(Error::Http("Comment too long".into()));
        }
        let user_id = claims
            .subject()
            .parse::<i32>()
            .map_err(|_| Error::Http("Invalid user id in token".into()))?;
        let user_name = claims
            .get_username()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "User".to_string());

        let now = now_secs();
        let blog_id = req.blog_id;
        let mut comment = req.to_entity(user_id, now);
        comment.user_name = user_name.clone();
        comment.content = content_owned;
        // 单 lock scope：insert + 回查在同一临界区内。
        // FIXME(framework): rust-ef 1.2.0 的 `save_changes` 不回填自增 id
        // (`execute_inserts` 的 `on_key_backfill` 回调以 `0` 占位，
        // `IAsyncConnection` 亦无 `last_insert_rowid()`)，
        // 故无法用 `find(inserted_id)` 精确回查。当前以 `max_by_key(c.id)`
        // 近似——per-request Scoped DbContext 消除了请求内竞态，
        // 但跨请求 DB 层竞态仍存在（并发同 (blog_id, user_id) 插入可能取到他人记录）。
        // 待框架暴露 last-insert-id 后改为 `find(id)`。
        let last = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Comment>().add(comment);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to create comment: {}", e)))?;
            linq!(ctx.set::<Comment>(), |c: Comment| c.blog_id == blog_id && c.user_id == user_id)
                .to_list()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
                .into_iter()
                .max_by_key(|c| c.id)
                .ok_or_else(|| Error::Internal("Comment disappeared after insert".into()))?
        };
        tracing::info!("[Comment] Created by {} on blog {}", user_name, blog_id);
        Ok(last.to_model())
    }
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<DeleteCommentRequest, String> for DeleteCommentHandler {
    async fn handle(&self, req: DeleteCommentRequest) -> Result<String> {
        let id = parse_id(&req.id)?;
        let mut comment = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Comment>()
                .query()
                .find(id)
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Comment not found".into()))?;

        // 鉴权：admin 或评论作者可删除
        let claims = req
            .claims
            .as_deref()
            .ok_or_else(|| Error::Http("Not authenticated".into()))?;
        let uid = claims
            .subject()
            .parse::<i32>()
            .map_err(|_| Error::Http("Invalid user id in token".into()))?;
        if !claims.has_role("admin") && comment.user_id != uid {
            return Err(Error::Http("Forbidden: can only delete your own comments".into()));
        }

        comment.is_deleted = true;
        comment.updated_id = operator_id(Some(claims));
        comment.updated_at = now_secs();
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Comment>().update(comment);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }
        tracing::info!("[Comment] Soft-deleted: {}", id);
        Ok(format!("Deleted comment {}", id))
    }
}
