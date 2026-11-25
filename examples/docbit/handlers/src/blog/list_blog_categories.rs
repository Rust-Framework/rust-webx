//! `GET /api/blog/categories` — categories with exact post counts.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::blog::{BlogCategoryCount, ListBlogCategoriesRequest};
use docbit_domain::entities::{Blog, Category};

use crate::common::MAX_LIST_ROWS;

#[derive(Inject)]
pub struct ListBlogCategoriesHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListBlogCategoriesRequest, Vec<BlogCategoryCount>>
    for ListBlogCategoriesHandler
{
    async fn handle(
        &mut self,
        _: ListBlogCategoriesRequest,
    ) -> ActionResult<Vec<BlogCategoryCount>> {
        let cats = linq!(self.ctx.set::<Category>(); order_by c.sort_order asc)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;

        // One COUNT per category rather than materialising the whole post table:
        // the counts stay exact and no query scans an unbounded result set.
        let mut result = Vec::with_capacity(cats.len());
        for c in cats {
            let q = c.id.clone();
            let count: i64 =
                linq!(self.ctx.set::<Blog>(), |b: Blog| b.category_id == q; count).await?;
            result.push(BlogCategoryCount {
                id: c.id,
                name: c.name,
                slug: c.slug,
                count: count as usize,
            });
        }

        Ok(result)
    }
}
