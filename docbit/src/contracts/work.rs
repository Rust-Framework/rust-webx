use rust_webapp::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkModel {
    pub slug: String,
    pub title: String,
    pub subtitle: String,
    pub description: String,
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
}

pub struct ListWorksRequest;

#[get("/api/works")]
impl IRequest<Vec<WorkModel>> for ListWorksRequest {}

pub struct GetWorkRequest {
    pub slug: String,
}

#[get("/api/works/{slug}")]
impl IRequest<WorkModel> for GetWorkRequest {}
