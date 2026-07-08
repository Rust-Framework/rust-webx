//! Exhibition entity — stores INDEX.json metadata for portfolio works.

use rust_ef::prelude::*;

use super::category::Category;

#[derive(Debug, Clone, EntityType)]
#[table("exhibitions")]
pub struct Exhibition {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[max_length(100)]
    #[unique]
    pub slug: String,
    #[required]
    #[max_length(200)]
    pub title: String,
    #[max_length(200)]
    pub subtitle: String,
    #[required]
    pub description: String,
    #[required]
    #[foreign_key(Category)]
    #[index]
    #[max_length(36)]
    pub category_id: String,
    #[required]
    pub tags: String,
    #[max_length(500)]
    pub repo_url: Option<String>,
    #[max_length(500)]
    pub demo_url: Option<String>,
    #[max_length(100)]
    pub docs_slug: Option<String>,
    #[required]
    pub featured: bool,
    #[required]
    pub sort_order: i32,
    #[max_length(500)]
    pub logo_url: Option<String>,
    #[required]
    pub created_at: i64,
    #[required]
    pub updated_at: i64,
    #[index]
    #[max_length(36)]
    pub created_id: Option<String>,
    #[index]
    #[max_length(36)]
    pub updated_id: Option<String>,
    #[required]
    #[index]
    pub is_deleted: bool,
    #[navigation]
    pub category: BelongsTo<Category>,
}
