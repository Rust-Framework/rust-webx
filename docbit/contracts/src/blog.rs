//! Blog contracts — DB-backed blog posts and category counts.
//!
//! Migrated from docbit/src/contracts/blog.rs with schema changes:
//! - `id` / `author_id` / `category_id` use String (UUID)
//! - `category: String` replaced by `category_id: String` + `category_name: String`
//! - `tags: Vec<String>` retained (serialized to JSON in entity)
//! - `published_at` / `created_at` / `updated_at` changed to i64 (Unix timestamps)
//!
//! 中介者模式下不需要 IBlogService 抽象 —— handler 直接使用 DbContext。

use rust_webx::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlogPostModel {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub tags: Vec<String>,
    pub category_id: String,
    pub category_name: String,
    pub author_id: String,
    pub author_name: String,
    pub published_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlogPostSummary {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub category_id: String,
    pub category_name: String,
    pub author_id: String,
    pub author_name: String,
    pub published_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlogCategoryCount {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub count: usize,
}

// ── HTTP requests ──

#[derive(Default, Deserialize)]
pub struct ListBlogPostsRequest;

#[get("/api/blog")]
impl IRequest<Vec<BlogPostSummary>> for ListBlogPostsRequest {}

#[derive(Default, Deserialize)]
pub struct ListBlogCategoriesRequest;

#[get("/api/blog/categories")]
impl IRequest<Vec<BlogCategoryCount>> for ListBlogCategoriesRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct ListMyBlogPostsRequest;

#[get("/api/blog/my")]
#[authorize]
impl IRequest<Vec<BlogPostSummary>> for ListMyBlogPostsRequest {}

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetBlogPostRequest {
    #[from_route]
    pub slug: String,
}

#[get("/api/blog/{slug}")]
impl IRequest<BlogPostModel> for GetBlogPostRequest {}

#[claims]
#[derive(Default, Deserialize)]
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

#[claims]
#[derive(Default, Deserialize)]
pub struct UpdateBlogPostRequest {
    #[serde(default)]
    pub slug: String,
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

#[claims]
#[derive(Default, Deserialize)]
pub struct DeleteBlogPostRequest {
    pub slug: String,
}

#[delete("/api/blog/{slug}")]
#[authorize]
impl IRequest<String> for DeleteBlogPostRequest {}
