//! Blog handlers — technical articles.

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*, provider::DbValue};
use rust_webapp::*;
use tokio::sync::Mutex;

use crate::contracts::blog::*;
use crate::domain::blog::{BlogPostEntity, BlogPostModel, BlogPostSummary};

fn tags_json(tags: &[String]) -> String {
    serde_json::to_string(tags).unwrap_or_else(|_| "[]".into())
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListBlogPostsRequest, Vec<BlogPostSummary>>)]
pub struct ListBlogPostsHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<GetBlogPostRequest, BlogPostModel>)]
pub struct GetBlogPostHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<CreateBlogPostRequest, BlogPostModel>)]
pub struct CreateBlogPostHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<UpdateBlogPostRequest, BlogPostModel>)]
pub struct UpdateBlogPostHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<DeleteBlogPostRequest, String>)]
pub struct DeleteBlogPostHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListBlogPostsRequest, Vec<BlogPostSummary>> for ListBlogPostsHandler {
    async fn handle(&self, _req: ListBlogPostsRequest) -> Result<Vec<BlogPostSummary>> {
        let query = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<BlogPostEntity>()
                .query()
                .order_by_desc_column("published_at")
        };
        let posts = query
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(posts.into_iter().map(BlogPostSummary::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetBlogPostRequest, BlogPostModel> for GetBlogPostHandler {
    async fn handle(&self, req: GetBlogPostRequest) -> Result<BlogPostModel> {
        let query = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<BlogPostEntity>()
                .query()
                .filter_column("slug", "=", DbValue::String(req.slug))
        };
        let post = query
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Blog post not found".into()))?;
        Ok(BlogPostModel::from(post))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateBlogPostRequest, BlogPostModel> for CreateBlogPostHandler {
    async fn handle(&self, req: CreateBlogPostRequest) -> Result<BlogPostModel> {
        let id = new_id();
        let now = now_secs();
        let sql = format!(
            "INSERT INTO blog_posts (id, slug, title, summary, content, tags, published_at, created_at) \
             VALUES ('{}', '{}', '{}', '{}', '{}', '{}', '{}', '{}')",
            id,
            crate::common::escape_sql(&req.slug),
            crate::common::escape_sql(&req.title),
            crate::common::escape_sql(&req.summary),
            crate::common::escape_sql(&req.content),
            crate::common::escape_sql(&tags_json(&req.tags)),
            crate::common::escape_sql(&req.published_at),
            now
        );
        {
            let ctx = self.ctx.lock().await;
            ctx.provider()
                .execute_migration_command(&sql)
                .await
                .map_err(|e| Error::Internal(format!("Failed to create blog post: {}", e)))?;
        }
        Ok(BlogPostModel {
            id,
            slug: req.slug,
            title: req.title,
            summary: req.summary,
            content: req.content,
            tags: req.tags,
            published_at: req.published_at,
            created_at: now,
        })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateBlogPostRequest, BlogPostModel> for UpdateBlogPostHandler {
    async fn handle(&self, req: UpdateBlogPostRequest) -> Result<BlogPostModel> {
        let query = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<BlogPostEntity>()
                .query()
                .filter_column("slug", "=", DbValue::String(req.slug.clone()))
        };
        let existing = query
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Blog post not found".into()))?;

        let title = req.title.unwrap_or(existing.title);
        let summary = req.summary.unwrap_or(existing.summary);
        let content = req.content.unwrap_or(existing.content);
        let tags = req
            .tags
            .map(|t| tags_json(&t))
            .unwrap_or(existing.tags);
        let published_at = req.published_at.unwrap_or(existing.published_at);

        let sql = format!(
            "UPDATE blog_posts SET title='{}', summary='{}', content='{}', tags='{}', published_at='{}' \
             WHERE slug='{}'",
            crate::common::escape_sql(&title),
            crate::common::escape_sql(&summary),
            crate::common::escape_sql(&content),
            crate::common::escape_sql(&tags),
            crate::common::escape_sql(&published_at),
            crate::common::escape_sql(&req.slug)
        );
        {
            let ctx = self.ctx.lock().await;
            ctx.provider()
                .execute_migration_command(&sql)
                .await
                .map_err(|e| Error::Internal(format!("Failed to update blog post: {}", e)))?;
        }
        Ok(BlogPostModel::from(BlogPostEntity {
            id: existing.id,
            slug: req.slug,
            title,
            summary,
            content,
            tags,
            published_at,
            created_at: existing.created_at,
        }))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteBlogPostRequest, String> for DeleteBlogPostHandler {
    async fn handle(&self, req: DeleteBlogPostRequest) -> Result<String> {
        let sql = format!(
            "DELETE FROM blog_posts WHERE slug='{}'",
            crate::common::escape_sql(&req.slug)
        );
        {
            let ctx = self.ctx.lock().await;
            ctx.provider()
                .execute_migration_command(&sql)
                .await
                .map_err(|e| Error::Internal(format!("Failed to delete blog post: {}", e)))?;
        }
        Ok(format!("Deleted blog post {}", req.slug))
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
