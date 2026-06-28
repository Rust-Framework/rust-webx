//! Comment handlers — list / create (with claims) / delete (soft or hard).
//!
//! Comment 的 `parent`/`quoted` 双自外键导航**不在 linq! 中 include**
//! （多 FK 导航绑定缺陷），列表仅返回扁平记录，前端按 id 二次拉取引用内容。

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*, provider::DbValue};
use rust_webapp::*;
use tokio::sync::Mutex;

use docbit_contracts::comment::{
    CommentModel, CreateCommentRequest, DeleteCommentRequest, ListCommentsRequest,
};
use docbit_domain::entities::Comment;

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

#[inject]
#[async_trait]
impl IRequestHandler<ListCommentsRequest, Vec<CommentModel>> for ListCommentsHandler {
    async fn handle(&self, req: ListCommentsRequest) -> Result<Vec<CommentModel>> {
        let blog_id = parse_id(&req.blog_id)?;
        let mut rows = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Comment>()
                .query()
                .filter_column("blog_id", "=", DbValue::I32(blog_id))
                .filter_column("is_deleted", "=", DbValue::Bool(false))
                .to_list()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        rows.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(rows.into_iter().map(CommentModel::from).collect())
    }
}

#[inject]
#[async_trait]
impl IRequestHandler<CreateCommentRequest, CommentModel> for CreateCommentHandler {
    async fn handle(&self, _: CreateCommentRequest) -> Result<CommentModel> {
        unreachable!("handle_with_claims is always called")
    }
    async fn handle_with_claims(
        &self,
        req: CreateCommentRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<CommentModel> {
        let claims = claims.ok_or_else(|| Error::Http("Not authenticated".into()))?;
        let content = req.content.trim();
        if content.is_empty() {
            return Err(Error::Http("Comment cannot be empty".into()));
        }
        if content.len() > 4000 {
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
        let comment = Comment {
            id: 0,
            blog_id: req.blog_id,
            user_id,
            user_name: user_name.clone(),
            content: content.to_string(),
            parent_id: req.parent_id,
            quoted_id: req.quoted_id,
            created_at: now,
            updated_id: Some(user_id),
            updated_at: now,
            is_deleted: false,
            blog: BelongsTo::new(),
            user: BelongsTo::new(),
            parent: BelongsTo::new(),
            quoted: BelongsTo::new(),
        };
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Comment>().add(comment);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to create comment: {}", e)))?;
        }
        let created = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Comment>()
                .query()
                .filter_column("blog_id", "=", DbValue::I32(req.blog_id))
                .filter_column("user_id", "=", DbValue::I32(user_id))
                .to_list()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        let last = created
            .into_iter()
            .max_by_key(|c| c.id)
            .ok_or_else(|| Error::Internal("Comment disappeared after insert".into()))?;
        tracing::info!("[Comment] Created by {} on blog {}", user_name, req.blog_id);
        Ok(CommentModel::from(last))
    }
}

#[inject]
#[async_trait]
impl IRequestHandler<DeleteCommentRequest, String> for DeleteCommentHandler {
    async fn handle(&self, _: DeleteCommentRequest) -> Result<String> {
        unreachable!("handle_with_claims is always called")
    }
    async fn handle_with_claims(
        &self,
        req: DeleteCommentRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<String> {
        let id = parse_id(&req.id)?;
        let mut comment = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Comment>()
                .query()
                .filter_column("id", "=", DbValue::I32(id))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Comment not found".into()))?;

        // 鉴权：admin 或评论作者可删除
        let claims = claims.ok_or_else(|| Error::Http("Not authenticated".into()))?;
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
