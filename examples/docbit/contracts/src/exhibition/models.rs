//! Exhibition read model — the portfolio works served by `/api/exhibitions`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExhibitionModel {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub category_id: String,
    pub category_name: String,
    pub category: String,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub demo_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs_slug: Option<String>,
    pub featured: bool,
    pub sort_order: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo_url: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
