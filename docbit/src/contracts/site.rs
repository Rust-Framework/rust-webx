use rust_webapp::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SiteConfig {
    pub title: String,
    pub tagline: String,
    pub author: String,
    pub bio: String,
    #[serde(default)]
    pub hero_subtitle: String,
    pub links: SiteLinks,
    #[serde(default)]
    pub stats: SiteStats,
    #[serde(default)]
    pub footer: SiteFooter,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SiteLinks {
    pub github: String,
    pub docs: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct SiteStats {
    #[serde(default = "default_stacks")]
    pub stacks: String,
    #[serde(default = "default_commits")]
    pub commits: String,
    #[serde(default = "default_rating")]
    pub rating: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct SiteFooter {
    #[serde(default = "default_motto")]
    pub motto: String,
    #[serde(default = "default_footer_tagline")]
    pub tagline: String,
    #[serde(default = "default_footer_stacks")]
    pub stacks: String,
    #[serde(default = "default_footer_exp")]
    pub experience: String,
    #[serde(default = "default_footer_years")]
    pub years: String,
    #[serde(default = "default_copyright")]
    pub copyright: String,
    #[serde(default = "default_icp")]
    pub icp: String,
    #[serde(default = "default_site_url")]
    pub site_url: String,
    #[serde(default = "default_site_label")]
    pub site_label: String,
}

fn default_stacks() -> String {
    "5".into()
}
fn default_commits() -> String {
    "2.1k+".into()
}
fn default_rating() -> String {
    "98%".into()
}
fn default_motto() -> String {
    "持续构建 · 不断探索 · 追求卓越".into()
}
fn default_footer_tagline() -> String {
    "用 Rust 构建更好的未来".into()
}
fn default_footer_stacks() -> String {
    "5+".into()
}
fn default_footer_exp() -> String {
    "100+".into()
}
fn default_footer_years() -> String {
    "2年+".into()
}
fn default_copyright() -> String {
    "© 2024 Start. All rights reserved.".into()
}
fn default_icp() -> String {
    "粤ICP备2023105607号-1".into()
}
fn default_site_url() -> String {
    "lusida.net".into()
}
fn default_site_label() -> String {
    "技术分享".into()
}

impl Default for SiteConfig {
    fn default() -> Self {
        Self {
            title: "Start 的作品".into(),
            tagline: "Rust · Web · Full Stack".into(),
            author: "Developer".into(),
            bio: "个人开发者作品集".into(),
            hero_subtitle: "构建高性能、可靠、未来感的 Web 应用".into(),
            links: SiteLinks {
                github: String::new(),
                docs: String::new(),
            },
            stats: SiteStats::default(),
            footer: SiteFooter::default(),
        }
    }
}

pub struct SiteInfoRequest;

#[get("/api/site")]
impl IRequest<SiteConfig> for SiteInfoRequest {}
