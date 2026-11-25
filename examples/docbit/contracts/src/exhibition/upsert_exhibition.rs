//! `POST /api/exhibitions` — create or replace one work's metadata (admin).

use serde::Deserialize;
use webx::*;

use super::models::ExhibitionModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
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
