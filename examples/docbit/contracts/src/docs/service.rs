//! `IDocumentService` — the documentation filesystem port.
//!
//! Implemented in the handlers layer (`docbit_handlers::doc_service::DocService`)
//! because it needs filesystem access and the app base path; consumers only see
//! this trait.

use std::path::{Path, PathBuf};

use crate::exhibition::ExhibitionModel;

use super::models::{DocContent, DocIndex};

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
