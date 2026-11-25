//! `POST /api/blog` — create a post (author = caller).

use serde::Deserialize;
use webx::*;

use super::models::BlogPostModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct CreateBlogPostRequest {
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub tags: Vec<String>,
    pub category_id: Option<String>,
    pub published_at: i64,
}

#[post("/api/blog")]
#[authorize]
impl IRequest<BlogPostModel> for CreateBlogPostRequest {}
