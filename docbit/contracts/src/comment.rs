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

pub struct ListCommentsRequest {
    pub blog_id: i32,
}

#[get("/api/comments")]
impl IRequest<Vec<CommentModel>> for ListCommentsRequest {}

#[derive(Deserialize)]
pub struct CreateCommentRequest {
    pub blog_id: i32,
    pub content: String,
    pub parent_id: Option<i32>,
    pub quoted_id: Option<i32>,
}

#[post("/api/comments")]
#[authorize]
impl IRequest<CommentModel> for CreateCommentRequest {}

pub struct DeleteCommentRequest {
    pub id: i32,
}

#[delete("/api/comments/{id}")]
#[authorize]
impl IRequest<String> for DeleteCommentRequest {}
