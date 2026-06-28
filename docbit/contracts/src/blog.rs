//! Blog contracts — DB-backed blog posts and category counts.
//!
//! Migrated from docbit/src/contracts/blog.rs with schema changes:
//! - `id` / `author_id` / `category_id` changed from String to i32
//! - `category: String` replaced by `category_id: i32` + `category_name: String`
//! - `tags: Vec<String>` retained (serialized to JSON in entity)
//! - `published_at` / `created_at` / `updated_at` changed to i64 (Unix timestamps)
//!
//! 中介者模式下不需要 IBlogService 抽象 —— handler 直接使用 DbContext。

use rust_webapp::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlogPostModel {
    pub id: i32,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub tags: Vec<String>,
    pub category_id: i32,
    pub category_name: String,
    pub author_id: i32,
    pub author_name: String,
    pub published_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlogPostSummary {
    pub id: i32,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub category_id: i32,
    pub category_name: String,
    pub author_id: i32,
    pub author_name: String,
    pub published_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlogCategoryCount {
    pub id: i32,
    pub name: String,
    pub slug: String,
    pub count: usize,
}

// ── HTTP requests ──

#[derive(Default)]
pub struct ListBlogPostsRequest;

#[get("/api/blog")]
impl IRequest<Vec<BlogPostSummary>> for ListBlogPostsRequest {}

#[derive(Default)]
pub struct ListBlogCategoriesRequest;

#[get("/api/blog/categories")]
impl IRequest<Vec<BlogCategoryCount>> for ListBlogCategoriesRequest {}

#[derive(Default)]
pub struct ListMyBlogPostsRequest {
    pub claims: Option<Box<dyn IClaims>>,
}
impl_claims_carrier!(ListMyBlogPostsRequest);

#[get("/api/blog/my")]
#[authorize]
impl IRequest<Vec<BlogPostSummary>> for ListMyBlogPostsRequest {}

#[derive(Default)]
pub struct GetBlogPostRequest {
    pub slug: String,
}

#[get("/api/blog/{slug}")]
impl IRequest<BlogPostModel> for GetBlogPostRequest {}

#[derive(Default, Deserialize)]
pub struct CreateBlogPostRequest {
    #[serde(skip)]
    pub claims: Option<Box<dyn IClaims>>,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub tags: Vec<String>,
    pub category_id: Option<i32>,
    pub published_at: i64,
}
impl_claims_carrier!(CreateBlogPostRequest);

#[post("/api/blog")]
#[authorize]
impl IRequest<BlogPostModel> for CreateBlogPostRequest {}

#[derive(Default, Deserialize)]
pub struct UpdateBlogPostRequest {
    #[serde(skip)]
    pub claims: Option<Box<dyn IClaims>>,
    pub slug: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub tags: Option<Vec<String>>,
    pub category_id: Option<i32>,
    pub published_at: Option<i64>,
}
impl_claims_carrier!(UpdateBlogPostRequest);

#[put("/api/blog/{slug}")]
#[authorize]
impl IRequest<BlogPostModel> for UpdateBlogPostRequest {}

#[derive(Default)]
pub struct DeleteBlogPostRequest {
    pub claims: Option<Box<dyn IClaims>>,
    pub slug: String,
}
impl_claims_carrier!(DeleteBlogPostRequest);

#[delete("/api/blog/{slug}")]
#[authorize]
impl IRequest<String> for DeleteBlogPostRequest {}
