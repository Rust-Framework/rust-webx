//! `GET /api/blog/my` — the caller's own posts, newest first.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::blog::{BlogPostSummary, ListMyBlogPostsRequest};
use docbit_domain::entities::Blog;

use crate::common::{operator_id, MAX_LIST_ROWS};

#[derive(Inject)]
pub struct ListMyBlogPostsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListMyBlogPostsRequest, Vec<BlogPostSummary>> for ListMyBlogPostsHandler {
    async fn handle(&mut self, _req: ListMyBlogPostsRequest) -> ActionResult<Vec<BlogPostSummary>> {
        let q = operator_id().ok_or_else(|| Error::unauthorized("Not authenticated"))?;

        let blogs = linq!(self.ctx.set::<Blog>(), |b: Blog| b.author_id == q; include b.category; include b.author; order_by b.published_at desc)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;

        Ok(blogs.iter().map(BlogPostSummary::from).collect())
    }
}
