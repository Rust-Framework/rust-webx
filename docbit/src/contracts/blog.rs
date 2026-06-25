use rust_webapp::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlogPostModel {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub tags: Vec<String>,
    pub category: String,
    pub published_at: String,
    pub created_at: String,
    pub author_id: String,
    pub author_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlogPostSummary {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub category: String,
    pub published_at: String,
    pub author_id: String,
    pub author_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlogCommentModel {
    pub id: String,
    pub post_slug: String,
    pub user_id: String,
    pub user_name: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlogCategoryDef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlogCategoryCount {
    pub id: String,
    pub name: String,
    pub count: usize,
}

/// Manages blog posts stored as `blog-data/{user_id}/INDEX.json` + markdown files.
pub trait IBlogService: Send + Sync {
    fn list_all_posts(&self) -> std::result::Result<Vec<BlogPostSummary>, String>;
    fn list_user_posts(&self, user_id: &str) -> std::result::Result<Vec<BlogPostSummary>, String>;
    fn list_categories(&self) -> std::result::Result<Vec<BlogCategoryCount>, String>;
    fn upsert_category(&self, id: &str, name: &str) -> std::result::Result<BlogCategoryDef, String>;
    fn get_post(&self, slug: &str) -> std::result::Result<BlogPostModel, String>;
    fn create_post(
        &self,
        user_id: &str,
        user_name: &str,
        slug: &str,
        title: &str,
        summary: &str,
        content: &str,
        tags: &[String],
        category: &str,
        published_at: &str,
    ) -> std::result::Result<BlogPostModel, String>;
    fn update_post(
        &self,
        actor_id: &str,
        actor_role: &str,
        slug: &str,
        title: Option<&str>,
        summary: Option<&str>,
        content: Option<&str>,
        tags: Option<&[String]>,
        category: Option<&str>,
        published_at: Option<&str>,
    ) -> std::result::Result<BlogPostModel, String>;
    fn delete_post(&self, actor_id: &str, actor_role: &str, slug: &str) -> std::result::Result<(), String>;
}

pub struct ListBlogPostsRequest;

#[get("/api/blog")]
impl IRequest<Vec<BlogPostSummary>> for ListBlogPostsRequest {}

pub struct ListBlogCategoriesRequest;

#[get("/api/blog/categories")]
impl IRequest<Vec<BlogCategoryCount>> for ListBlogCategoriesRequest {}

#[derive(Deserialize)]
pub struct CreateBlogCategoryRequest {
    pub id: String,
    pub name: String,
}

#[post("/api/blog/categories")]
#[authorize]
impl IRequest<BlogCategoryDef> for CreateBlogCategoryRequest {}

pub struct ListMyBlogPostsRequest;

#[get("/api/blog/my")]
#[authorize]
impl IRequest<Vec<BlogPostSummary>> for ListMyBlogPostsRequest {}

pub struct GetBlogPostRequest {
    pub slug: String,
}

#[get("/api/blog/{slug}")]
impl IRequest<BlogPostModel> for GetBlogPostRequest {}

pub struct ListBlogCommentsRequest {
    pub slug: String,
}

#[get("/api/blog/{slug}/comments")]
impl IRequest<Vec<BlogCommentModel>> for ListBlogCommentsRequest {}

#[derive(Deserialize)]
pub struct CreateBlogCommentRequest {
    pub slug: String,
    pub content: String,
}

#[post("/api/blog/{slug}/comments")]
#[authorize]
impl IRequest<BlogCommentModel> for CreateBlogCommentRequest {}

#[derive(Deserialize)]
pub struct CreateBlogPostRequest {
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub tags: Vec<String>,
    pub category: Option<String>,
    pub published_at: String,
}

#[post("/api/blog")]
#[authorize]
impl IRequest<BlogPostModel> for CreateBlogPostRequest {}

#[derive(Deserialize)]
pub struct UpdateBlogPostRequest {
    pub slug: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub tags: Option<Vec<String>>,
    pub category: Option<String>,
    pub published_at: Option<String>,
}

#[put("/api/blog/{slug}")]
#[authorize]
impl IRequest<BlogPostModel> for UpdateBlogPostRequest {}

pub struct DeleteBlogPostRequest {
    pub slug: String,
}

#[delete("/api/blog/{slug}")]
#[authorize]
impl IRequest<String> for DeleteBlogPostRequest {}
