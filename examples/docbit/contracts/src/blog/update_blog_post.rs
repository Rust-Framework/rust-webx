//! `PUT /api/blog/{slug}` — partial update (author or admin).

use serde::Deserialize;
use webx::*;

use super::models::BlogPostModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct UpdateBlogPostRequest {
    #[frompath]
    #[serde(default)]
    pub slug: String,
    #[fromclaim]
    pub user: IdentityUser,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub tags: Option<Vec<String>>,
    pub category_id: Option<String>,
    pub published_at: Option<i64>,
}

#[put("/api/blog/{slug}")]
#[authorize]
impl IRequest<BlogPostModel> for UpdateBlogPostRequest {}
