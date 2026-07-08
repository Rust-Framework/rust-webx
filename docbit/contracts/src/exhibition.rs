//! Exhibition contracts — replaces work.rs; stores INDEX.json metadata for searchability.

use rust_webx::*;
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

#[derive(Default)]
pub struct ListExhibitionsRequest;

#[get("/api/exhibitions")]
impl IRequest<Vec<ExhibitionModel>> for ListExhibitionsRequest {}

#[derive(Default)]
pub struct GetExhibitionRequest {
    pub slug: String,
}

#[get("/api/exhibitions/{slug}")]
impl IRequest<ExhibitionModel> for GetExhibitionRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct UpsertExhibitionRequest {
    pub slug: String,
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub category_id: String,
    pub tags: Vec<String>,
    pub repo_url: Option<String>,
    pub demo_url: Option<String>,
    pub docs_slug: Option<String>,
    pub featured: bool,
    pub sort_order: i32,
    pub logo_url: Option<String>,
}

#[post("/api/exhibitions")]
#[authorize(role = "admin")]
impl IRequest<ExhibitionModel> for UpsertExhibitionRequest {}

#[claims]
#[derive(Default)]
pub struct DeleteExhibitionRequest {
    pub slug: String,
}

#[delete("/api/exhibitions/{slug}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteExhibitionRequest {}
