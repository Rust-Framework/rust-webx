//! Documentation contracts — filesystem-based doc reading service.
//!
//! Migrated from docbit/src/contracts/docs.rs. The `list_portfolio` / `get_portfolio`
//! methods now return `ExhibitionModel` (from the exhibitions DB table) instead of
//! the legacy `WorkModel`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use webx::*;

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

    /// Writable directory an uploaded bundle for `work` is installed into.
    ///
    /// Always `<app_base>/docs/{work}` — never the workspace mirror or a live
    /// sibling checkout, so an upload can never overwrite a source repository.
    fn deploy_dir(&self, work: &str) -> PathBuf;

    /// Atomically swap `deploy_dir(work)` for the tree in `staging`.
    ///
    /// The previous tree is moved aside first and only removed once the swap
    /// succeeds, so a failed install leaves the served documentation intact.
    /// Returns the installed directory.
    fn install_work_dir(&self, work: &str, staging: &Path) -> std::result::Result<PathBuf, String>;

    /// Whether `slug` is a known work (served from disk, with an INDEX.json).
    fn work_exists(&self, slug: &str) -> bool {
        self.index(slug).is_ok()
    }
}

#[derive(Default, Deserialize)]
pub struct ListDocWorksRequest;

#[get("/api/docs")]
impl IRequest<Vec<String>> for ListDocWorksRequest {}

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetDocIndexRequest {
    #[from_route]
    pub work: String,
}

#[get("/api/docs/{work}/index")]
impl IRequest<DocIndex> for GetDocIndexRequest {}

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct GetDocContentRequest {
    #[from_route]
    pub work: String,
    /// Remaining document path (slashes allowed via `{*path}`).
    /// Legacy colon encoding (`01-introduction:hello.md`) is still accepted.
    #[from_route]
    pub path: String,
}

#[get("/api/docs/{work}/content/{*path}")]
impl IRequest<DocContent> for GetDocContentRequest {}

/// What a catalog re-sync changed. Returned by the upload endpoint so the
/// operator sees the effect immediately instead of reading logs.
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

/// Replace one work's documentation with an uploaded zip bundle.
///
/// The archive root must contain `INDEX.json` (and the markdown tree it
/// describes). Extraction is sandboxed; see `docbit_handlers::works`.
#[derive(Deserialize, WebxRequestMeta)]
pub struct UploadWorkDocsRequest {
    #[from_route]
    pub slug: String,
    /// The `docs.zip` body part.
    pub archive: FormFile,
}

#[post("/api/works/{slug}/docs")]
#[authorize(role = "admin")]
impl IRequest<UploadWorkDocsResponse> for UploadWorkDocsRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadWorkDocsResponse {
    pub slug: String,
    /// Directory the bundle was installed into.
    pub installed_dir: String,
    /// Files written (after the swap).
    pub files: usize,
    /// Uncompressed bytes written.
    pub bytes: u64,
    /// Result of the catalog re-sync that followed the install.
    pub catalog: ResyncSummary,
}
