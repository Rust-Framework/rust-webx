//! `GET /api/comments/{blog_id}` — flat comment list, oldest first.
//!
//! `parent` / `quoted` self-navigations are deliberately **not** included here
//! (multi-FK navigation binding); the list returns flat rows and the client
//! fetches a quoted comment by id when it renders one.

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::comment::{CommentModel, ListCommentsRequest};
use docbit_domain::entities::Comment;

use crate::common::{parse_id, MAX_LIST_ROWS};

#[derive(Inject)]
pub struct ListCommentsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListCommentsRequest, Vec<CommentModel>> for ListCommentsHandler {
    async fn handle(&mut self, req: ListCommentsRequest) -> ActionResult<Vec<CommentModel>> {
        let q = parse_id(&req.blog_id)?;

        let rows = linq!(self.ctx.set::<Comment>(), |c: Comment| c.blog_id == q; order_by c.created_at asc)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;

        Ok(rows.iter().map(CommentModel::from).collect())
    }
}
