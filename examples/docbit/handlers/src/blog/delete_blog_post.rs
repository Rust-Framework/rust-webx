//! `DELETE /api/blog/{slug}` — soft delete; author or admin only.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::blog::DeleteBlogPostRequest;
use docbit_domain::entities::Blog;

use crate::common::{now_secs, operator_id, save_changes};

#[derive(Inject)]
pub struct DeleteBlogPostHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteBlogPostRequest, String> for DeleteBlogPostHandler {
    async fn handle(&mut self, req: DeleteBlogPostRequest) -> ActionResult<String> {
        // ── 身份与归属 ──
        let user = req.require_user()?;
        let uid = user.id.clone();
        let is_admin = user.is_in_role("admin");

        // ── 加载 ──
        let q = req.slug.clone();
        let blog = linq!(self.ctx.set::<Blog>(), |b: Blog| b.slug == q)
            .first_or_default()
            .await?
            .ok_or_else(|| Error::not_found(format!("Blog post not found: {}", req.slug)))?;

        if !is_admin && blog.author_id != uid {
            return Err(Error::forbidden("not the author"));
        }

        // ── 变更与持久化 ──
        self.ctx.edit(blog, |b| {
            b.is_deleted = true;
            b.updated_id = operator_id();
            b.updated_at = now_secs();
        });
        self.ctx.detect_changes();

        save_changes(&mut self.ctx).await?;

        tracing::info!("[Blog] Soft-deleted: {}", req.slug);
        Ok(format!("Deleted blog post {}", req.slug))
    }
}
