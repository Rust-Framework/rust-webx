//! `GET /api/blog` — public post list, newest first (no `content`).

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::blog::{BlogPostSummary, ListBlogPostsRequest};
use docbit_domain::entities::Blog;

use crate::common::MAX_LIST_ROWS;

#[derive(Inject)]
pub struct ListBlogPostsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListBlogPostsRequest, Vec<BlogPostSummary>> for ListBlogPostsHandler {
    async fn handle(&mut self, _: ListBlogPostsRequest) -> ActionResult<Vec<BlogPostSummary>> {
        let blogs = linq!(self.ctx.set::<Blog>(); include b.category; include b.author; order_by b.published_at desc)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;

        Ok(blogs.iter().map(BlogPostSummary::from).collect())
    }
}
