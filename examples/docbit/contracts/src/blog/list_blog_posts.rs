//! `GET /api/blog` — public post list (no `content`).

use serde::Deserialize;
use webx::*;

use super::models::BlogPostSummary;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListBlogPostsRequest;

#[get("/api/blog")]
impl IRequest<Vec<BlogPostSummary>> for ListBlogPostsRequest {}
