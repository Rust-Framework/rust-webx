//! Exhibition entity — stores INDEX.json metadata for portfolio works.

use rust_ef::prelude::*;
use serde::{Deserialize, Serialize};

use super::category::Category;

#[derive(Debug, Clone, EntityType, Serialize, Deserialize)]
#[table("exhibitions")]
pub struct Exhibition {
    #[primary_key]
    #[auto_increment]
    pub id: i32,
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
    pub category_id: i32,
    #[required]
    pub tags: String, // JSON 数组字符串
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
    #[navigation]
    pub category: BelongsTo<Category>,
}
