//! Blog comment handlers.

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*, provider::DbValue};
use rust_webapp::*;
use tokio::sync::Mutex;

use crate::common::escape_sql;
use crate::contracts::blog::*;
use crate::contracts::blog::BlogCommentModel;
use crate::domain::comment::BlogCommentEntity;

#[rust_dicore::inject_attr(
    singleton,
    as = dyn IRequestHandler<ListBlogCommentsRequest, Vec<BlogCommentModel>>
)]
pub struct ListBlogCommentsHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(
    singleton,
    as = dyn IRequestHandler<CreateBlogCommentRequest, BlogCommentModel>
)]
pub struct CreateBlogCommentHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListBlogCommentsRequest, Vec<BlogCommentModel>> for ListBlogCommentsHandler {
    async fn handle(&self, req: ListBlogCommentsRequest) -> Result<Vec<BlogCommentModel>> {
        let query = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<BlogCommentEntity>()
                .query()
                .filter_column("post_slug", "=", DbValue::String(req.slug))
        };
        let mut rows = query
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        rows.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(rows.into_iter().map(BlogCommentModel::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateBlogCommentRequest, BlogCommentModel> for CreateBlogCommentHandler {
    async fn handle(&self, _: CreateBlogCommentRequest) -> Result<BlogCommentModel> {
        unreachable!("handle_with_claims is always called")
    }

    async fn handle_with_claims(
        &self,
        req: CreateBlogCommentRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<BlogCommentModel> {
        let claims = claims.ok_or_else(|| Error::Http("Not authenticated".into()))?;
        let content = req.content.trim();
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

        let id = new_id();
        let now = now_secs();
        let sql = format!(
            "INSERT INTO blog_comments (id, post_slug, user_id, user_name, content, created_at) \
             VALUES ('{}', '{}', '{}', '{}', '{}', '{}')",
            id,
            escape_sql(&req.slug),
            escape_sql(&user_id),
            escape_sql(&user_name),
            escape_sql(content),
            now
        );
        {
            let ctx = self.ctx.lock().await;
            ctx.provider()
                .execute_migration_command(&sql)
                .await
                .map_err(|e| Error::Internal(format!("Failed to create comment: {}", e)))?;
        }

        Ok(BlogCommentModel {
            id,
            post_slug: req.slug,
            user_id,
            user_name,
            content: content.to_string(),
            created_at: now,
        })
    }
}

fn new_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| format!("{:x}", d.as_nanos()))
        .unwrap_or_else(|_| "0".to_string())
}

fn now_secs() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}
