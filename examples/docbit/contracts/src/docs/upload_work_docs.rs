//! `POST /api/works/{slug}/docs` — replace one work's documentation with an
//! uploaded zip bundle (admin only).

use serde::{Deserialize, Serialize};
use webx::*;

use super::models::ResyncSummary;

/// Replace one work's documentation with an uploaded zip bundle.
///
/// The archive root must contain `INDEX.json` (and the markdown tree it
/// describes). Extraction is sandboxed; see `docbit_handlers::docs::extract`.
#[derive(WebxRequestMeta)]
pub struct UploadWorkDocsRequest {
    #[frompath]
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
