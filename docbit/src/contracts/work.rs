use crate::domain::work::WorkModel;
use rust_webapp::*;

pub struct ListWorksRequest;

#[get("/api/works")]
impl IRequest<Vec<WorkModel>> for ListWorksRequest {}

pub struct GetWorkRequest {
    pub slug: String,
}

#[get("/api/works/{slug}")]
impl IRequest<WorkModel> for GetWorkRequest {}

#[derive(serde::Deserialize)]
pub struct CreateWorkRequest {
    pub slug: String,
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub category: String,
    pub tags: Vec<String>,
    pub repo_url: Option<String>,
    pub demo_url: Option<String>,
    pub docs_slug: Option<String>,
    pub featured: bool,
    pub sort_order: i32,
}

#[post("/api/works")]
#[authorize(role = "admin")]
impl IRequest<WorkModel> for CreateWorkRequest {}

#[derive(serde::Deserialize)]
pub struct UpdateWorkRequest {
    pub slug: String,
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub tags: Option<Vec<String>>,
    pub repo_url: Option<String>,
    pub demo_url: Option<String>,
    pub docs_slug: Option<String>,
    pub featured: Option<bool>,
    pub sort_order: Option<i32>,
}

#[put("/api/works/{slug}")]
#[authorize(role = "admin")]
impl IRequest<WorkModel> for UpdateWorkRequest {}

pub struct DeleteWorkRequest {
    pub slug: String,
}

#[delete("/api/works/{slug}")]
#[authorize(role = "admin")]
impl IRequest<String> for DeleteWorkRequest {}
