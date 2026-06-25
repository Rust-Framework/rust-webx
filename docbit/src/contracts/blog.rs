use crate::domain::blog::{BlogPostModel, BlogPostSummary};
use rust_webapp::*;

pub struct ListBlogPostsRequest;

#[get("/api/blog")]
impl IRequest<Vec<BlogPostSummary>> for ListBlogPostsRequest {}

pub struct GetBlogPostRequest {
    pub slug: String,
}

#[get("/api/blog/{slug}")]
impl IRequest<BlogPostModel> for GetBlogPostRequest {}

#[derive(serde::Deserialize)]
pub struct CreateBlogPostRequest {
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub tags: Vec<String>,
    pub published_at: String,
}

#[post("/api/blog")]
#[authorize(role = "admin")]
impl IRequest<BlogPostModel> for CreateBlogPostRequest {}

#[derive(serde::Deserialize)]
pub struct UpdateBlogPostRequest {
    pub slug: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub tags: Option<Vec<String>>,
    pub published_at: Option<String>,
}

#[put("/api/blog/{slug}")]
#[authorize(role = "admin")]
impl IRequest<BlogPostModel> for UpdateBlogPostRequest {}

pub struct DeleteBlogPostRequest {
    pub slug: String,
}

#[delete("/api/blog/{slug}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteBlogPostRequest {}
