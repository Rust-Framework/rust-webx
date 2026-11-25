//! Comment read model — supports reply (`parent_id`) and quote (`quoted_id`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentModel {
    pub id: String,
    pub blog_id: String,
    pub user_id: String,
    pub user_name: String,
    pub content: String,
    pub parent_id: Option<String>,
    pub quoted_id: Option<String>,
    pub created_at: i64,
}
