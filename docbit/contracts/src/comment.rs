//! Comment contracts — supports reply (`parent_id`) and quote (`quoted_id`).

use rust_webx::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentModel {
    pub id: String,
    pub blog_id: String,
    pub user_id: String,
    pub user_name: String,
    pub content: String,
    pub parent_id: Option<String>,
    pub quoted_id: Option<String>,
    pub created_at: i64,
}

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListCommentsRequest {
    #[from_route]
    pub blog_id: String,
}

#[get("/api/comments/{blog_id}")]
impl IRequest<Vec<CommentModel>> for ListCommentsRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct CreateCommentRequest {
    pub blog_id: String,
    pub content: String,
    pub parent_id: Option<String>,
    pub quoted_id: Option<String>,
}

#[post("/api/comments")]
#[authorize]
impl IRequest<CommentModel> for CreateCommentRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct DeleteCommentRequest {
    pub id: String,
}

#[delete("/api/comments/{id}")]
#[authorize]
impl IRequest<String> for DeleteCommentRequest {}
