//! Site display options (`Site` configuration section).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SiteConfig {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub brand_name: String,
    #[serde(default)]
    pub room_name: String,
}
