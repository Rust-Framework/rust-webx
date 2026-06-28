//! Comment contracts — supports reply (`parent_id`) and quote (`quoted_id`).

use rust_webapp::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentModel {
    pub id: i32,
    pub blog_id: i32,
    pub user_id: i32,
    pub user_name: String,
    pub content: String,
    pub parent_id: Option<i32>,
    pub quoted_id: Option<i32>,
    pub created_at: i64,
}

#[derive(Default)]
pub struct ListCommentsRequest {
    pub blog_id: String,
}

#[get("/api/comments/{blog_id}")]
impl IRequest<Vec<CommentModel>> for ListCommentsRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct CreateCommentRequest {
    pub blog_id: i32,
    pub content: String,
    pub parent_id: Option<i32>,
    pub quoted_id: Option<i32>,
}

#[post("/api/comments")]
#[authorize]
impl IRequest<CommentModel> for CreateCommentRequest {}

#[claims]
#[derive(Default)]
pub struct DeleteCommentRequest {
    pub id: String,
}

#[delete("/api/comments/{id}")]
#[authorize]
impl IRequest<String> for DeleteCommentRequest {}
