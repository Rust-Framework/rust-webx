//! Exhibition contracts — replaces work.rs; stores INDEX.json metadata for searchability.

use rust_webapp::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExhibitionModel {
    pub id: i32,
    pub slug: String,
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub category_id: i32,
    pub category_name: String,
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

pub struct ListExhibitionsRequest;

#[get("/api/exhibitions")]
impl IRequest<Vec<ExhibitionModel>> for ListExhibitionsRequest {}

pub struct GetExhibitionRequest {
    pub slug: String,
}

#[get("/api/exhibitions/{slug}")]
impl IRequest<ExhibitionModel> for GetExhibitionRequest {}

#[derive(Deserialize)]
pub struct UpsertExhibitionRequest {
    pub slug: String,
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub category_id: i32,
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
