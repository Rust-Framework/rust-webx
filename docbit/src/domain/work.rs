use rust_ef::prelude::*;
use serde::{Deserialize, Serialize};

/// Portfolio work entity — frameworks, products, articles, etc.
#[derive(EntityType, Clone, Serialize, Deserialize, Debug)]
#[table("works")]
pub struct WorkEntity {
    #[primary_key]
    pub id: String,
    #[max_length(100)]
    pub slug: String,
    #[max_length(200)]
    pub title: String,
    #[max_length(300)]
    pub subtitle: String,
    pub description: String,
    #[max_length(50)]
    pub category: String,
    pub tags: String,
    #[max_length(500)]
    pub repo_url: String,
    #[max_length(500)]
    pub demo_url: String,
    #[max_length(100)]
    pub docs_slug: String,
    pub featured: i32,
    pub sort_order: i32,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkModel {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub category: String,
    pub tags: Vec<String>,
    pub repo_url: Option<String>,
    pub demo_url: Option<String>,
    pub docs_slug: Option<String>,
    pub featured: bool,
    pub sort_order: i32,
    pub created_at: String,
}

impl From<WorkEntity> for WorkModel {
    fn from(e: WorkEntity) -> Self {
        let tags = serde_json::from_str(&e.tags).unwrap_or_default();
        Self {
            id: e.id,
            slug: e.slug,
            title: e.title,
            subtitle: e.subtitle,
            description: e.description,
            category: e.category,
            tags,
            repo_url: opt_str(e.repo_url),
            demo_url: opt_str(e.demo_url),
            docs_slug: opt_str(e.docs_slug),
            featured: e.featured != 0,
            sort_order: e.sort_order,
            created_at: e.created_at,
        }
    }
}

fn opt_str(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}
