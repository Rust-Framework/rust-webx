//! Documentation read models — the generated index, one markdown page, and the
//! catalog re-sync summary that an upload reports back to the operator.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocIndex {
    pub title: String,
    pub items: Vec<DocIndexItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocIndexItem {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<DocIndexItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocContent {
    pub path: String,
    pub content: String,
}

/// What a catalog re-sync changed, so an operator sees the effect immediately
/// instead of reading logs.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct ResyncSummary {
    /// Documentation directories discovered.
    pub works: usize,
    /// Existing works whose metadata changed from INDEX.json.
    pub updated: usize,
    /// Works created from a seed template.
    pub inserted: usize,
    /// Logo files present under `wwwroot/assets/works` afterwards.
    pub logos: usize,
}
