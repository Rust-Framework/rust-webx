use rust_ef::prelude::*;
use serde::{Deserialize, Serialize};

/// Blog post entity for technical articles.
#[derive(EntityType, Clone, Serialize, Deserialize, Debug)]
#[table("blog_posts")]
pub struct BlogPostEntity {
    #[primary_key]
    pub id: String,
    #[max_length(120)]
    pub slug: String,
    #[max_length(300)]
    pub title: String,
    pub summary: String,
    pub content: String,
    pub tags: String,
    pub published_at: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlogPostModel {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub tags: Vec<String>,
    pub published_at: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlogPostSummary {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub published_at: String,
}

impl From<BlogPostEntity> for BlogPostModel {
    fn from(e: BlogPostEntity) -> Self {
        let tags = serde_json::from_str(&e.tags).unwrap_or_default();
        Self {
            id: e.id,
            slug: e.slug,
            title: e.title,
            summary: e.summary,
            content: e.content,
            tags,
            published_at: e.published_at,
            created_at: e.created_at,
        }
    }
}

impl From<BlogPostEntity> for BlogPostSummary {
    fn from(e: BlogPostEntity) -> Self {
        let tags = serde_json::from_str(&e.tags).unwrap_or_default();
        Self {
            id: e.id,
            slug: e.slug,
            title: e.title,
            summary: e.summary,
            tags,
            published_at: e.published_at,
        }
    }
}
