//! `PUT /api/blog/{slug}` — partial update; author or admin only.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::blog::{BlogPostModel, UpdateBlogPostRequest};
use docbit_domain::entities::Blog;

use crate::common::save_changes;

#[derive(Inject)]
pub struct UpdateBlogPostHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateBlogPostRequest, BlogPostModel> for UpdateBlogPostHandler {
    async fn handle(&mut self, req: UpdateBlogPostRequest) -> ActionResult<BlogPostModel> {
        // ── 身份与归属 ──
        let user = req.require_user()?;
        let uid = user.id.clone();
        let is_admin = user.is_in_role("admin");

        // ── 加载 ──
        let q = req.slug.clone();
        let blog = linq!(self.ctx.set::<Blog>(), |b: Blog| b.slug == q; include b.category; include b.author)
            .first_or_default()
            .await?
            .ok_or_else(|| Error::not_found(format!("Blog post not found: {}", req.slug)))?;

        if !is_admin && blog.author_id != uid {
            return Err(Error::forbidden("not the author"));
        }

        // ── 变更与持久化 ──
        let blog_id = blog.id.clone();
        self.ctx.edit(blog, |b| b.patch(req));
        self.ctx.detect_changes();

        save_changes(&mut self.ctx).await?;

        // ── 响应 ──
        let saved = crate::ef_require_by_id!(
            self.ctx,
            Blog,
            blog_id,
            Error::not_found("Blog not found after update");
            include row.category;
            include row.author
        );

        Ok(BlogPostModel::from(&saved))
    }
}
