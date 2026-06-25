use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteConfig {
    pub title: String,
    pub tagline: String,
    pub author: String,
    pub bio: String,
    pub links: SiteLinks,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteLinks {
    pub github: String,
    pub docs: String,
}

impl Default for SiteConfig {
    fn default() -> Self {
        Self {
            title: "Start 的作品".into(),
            tagline: "Rust · Web · Full Stack".into(),
            author: "Developer".into(),
            bio: "个人开发者作品集".into(),
            links: SiteLinks {
                github: String::new(),
                docs: String::new(),
            },
        }
    }
}

pub fn load_site_config() -> SiteConfig {
    let path = crate::paths::resolve_data_path("appsettings.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
    parsed
        .get("Site")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}
