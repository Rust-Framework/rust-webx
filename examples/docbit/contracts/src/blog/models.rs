//! Blog read models — posts, list summaries, and per-category counts.

use serde::{Deserialize, Serialize};

/// A full blog post, including `content`.
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

/// A blog post without `content`, for list endpoints.
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

/// One blog category and how many posts it holds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlogCategoryCount {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub count: usize,
}
