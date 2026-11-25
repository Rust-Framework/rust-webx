//! `GET /api/blog/{slug}` — one post, with category and author names.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::blog::{BlogPostModel, GetBlogPostRequest};
use docbit_domain::entities::Blog;

#[derive(Inject)]
pub struct GetBlogPostHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetBlogPostRequest, BlogPostModel> for GetBlogPostHandler {
    async fn handle(&mut self, req: GetBlogPostRequest) -> ActionResult<BlogPostModel> {
        let q = req.slug.clone();

        let blog = linq!(self.ctx.set::<Blog>(), |b: Blog| b.slug == q; include b.category; include b.author)
            .first_or_default()
            .await?
            .ok_or_else(|| Error::not_found(format!("Blog post not found: {}", req.slug)))?;

        Ok(BlogPostModel::from(&blog))
    }
}
