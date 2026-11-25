//! `POST /api/comments` — any signed-in user.

use serde::Deserialize;
use webx::*;

use super::models::CommentModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct CreateCommentRequest {
    #[fromclaim]
    pub user: IdentityUser,
    pub blog_id: String,
    pub content: String,
    pub parent_id: Option<String>,
    pub quoted_id: Option<String>,
}

#[post("/api/comments")]
#[authorize]
impl IRequest<CommentModel> for CreateCommentRequest {}
