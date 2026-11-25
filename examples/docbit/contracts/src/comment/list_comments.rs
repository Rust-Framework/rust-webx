//! `GET /api/comments/{blog_id}` — flat comment list for one post.

use serde::Deserialize;
use webx::*;

use super::models::CommentModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListCommentsRequest {
    #[frompath]
    pub blog_id: String,
}

#[get("/api/comments/{blog_id}")]
impl IRequest<Vec<CommentModel>> for ListCommentsRequest {}
