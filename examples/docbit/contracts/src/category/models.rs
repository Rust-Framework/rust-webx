//! Category read models — flat node and the assembled tree node.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryModel {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub parent_id: Option<String>,
    pub sort_order: i32,
    pub created_at: i64,
}

/// A category plus its descendants, flattened for the tree response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryTreeNode {
    #[serde(flatten)]
    pub category: CategoryModel,
    pub children: Vec<CategoryTreeNode>,
    pub level: u32,
}
