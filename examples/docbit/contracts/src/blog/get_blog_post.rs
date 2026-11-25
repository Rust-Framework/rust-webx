//! `GET /api/blog/{slug}` — one post by slug.

use serde::Deserialize;
use webx::*;

use super::models::BlogPostModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetBlogPostRequest {
    #[frompath]
    pub slug: String,
}

#[get("/api/blog/{slug}")]
impl IRequest<BlogPostModel> for GetBlogPostRequest {}
