//! `POST /api/blog` — create a post; the caller becomes the author.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::blog::{BlogPostModel, CreateBlogPostRequest};
use docbit_domain::entities::Blog;

use crate::common::{operator_id, save_changes};

#[derive(Inject)]
pub struct CreateBlogPostHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateBlogPostRequest, BlogPostModel> for CreateBlogPostHandler {
    async fn handle(&mut self, req: CreateBlogPostRequest) -> ActionResult<BlogPostModel> {
        let uid = operator_id().ok_or_else(|| Error::unauthorized("Not authenticated"))?;

        // ── 校验：slug 全局唯一 ──
        let q = req.slug.clone();
        let exists = linq!(self.ctx.set::<Blog>(), |b: Blog| b.slug == q)
            .any()
            .await?;
        if exists {
            return Err(Error::conflict(format!(
                "Slug already exists: {}",
                req.slug
            )));
        }

        // ── 变更 ──
        let entity = Blog::try_from(req)?;
        let id = entity.id.clone();
        self.ctx.add(entity);

        save_changes(&mut self.ctx).await?;

        // ── 响应：带导航重新加载 ──
        let saved = crate::ef_require_by_id!(
            self.ctx,
            Blog,
            id,
            Error::not_found("Blog not found after save");
            include row.category;
            include row.author
        );

        tracing::info!("[Blog] Created: {} by {}", saved.slug, uid);
        Ok(BlogPostModel::from(&saved))
    }
}
