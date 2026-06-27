//! Documentation contracts — filesystem-based doc reading service.
//!
//! Migrated from docbit/src/contracts/docs.rs. The `list_portfolio` / `get_portfolio`
//! methods now return `ExhibitionModel` (from the exhibitions DB table) instead of
//! the legacy `WorkModel`.

use std::path::Path;

use rust_webapp::*;
use serde::{Deserialize, Serialize};

use crate::exhibition::ExhibitionModel;

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

/// Reads and indexes markdown documentation under `docs/{work}/`.
pub trait IDocumentService: Send + Sync {
    fn list_works(&self) -> std::result::Result<Vec<String>, String>;
    fn index(&self, work: &str) -> std::result::Result<DocIndex, String>;
    fn content(&self, work: &str, path: &str) -> std::result::Result<DocContent, String>;
    fn list_portfolio(&self) -> std::result::Result<Vec<ExhibitionModel>, String>;
    fn get_portfolio(&self, slug: &str) -> std::result::Result<ExhibitionModel, String>;
    fn ensure_all_indexes(&self) -> std::result::Result<(), String>;
    fn sync_portfolio_assets(&self, wwwroot: &Path) -> std::result::Result<(), String>;
}

pub struct ListDocWorksRequest;

#[get("/api/docs")]
impl IRequest<Vec<String>> for ListDocWorksRequest {}

pub struct GetDocIndexRequest {
    pub work: String,
}

#[get("/api/docs/{work}/index")]
impl IRequest<DocIndex> for GetDocIndexRequest {}

pub struct GetDocContentRequest {
    pub work: String,
    /// Document path with `/` encoded as `:` (e.g. `01-introduction:hello-world.md`)
    pub path: String,
}

#[get("/api/docs/{work}/content/{path}")]
impl IRequest<DocContent> for GetDocContentRequest {}
