//! `GET /api/blog/my` — the caller's own posts.

use serde::Deserialize;
use webx::*;

use super::models::BlogPostSummary;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListMyBlogPostsRequest;

#[get("/api/blog/my")]
#[authorize]
impl IRequest<Vec<BlogPostSummary>> for ListMyBlogPostsRequest {}
