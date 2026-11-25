//! `POST /api/works/{slug}/docs` — install an uploaded documentation bundle.
//!
//! Flow:
//!
//! 1. spool the uploaded archive to a temporary file under `uploads/.tmp`
//! 2. extract it into a staging directory, sandboxed (see [`super::extract`])
//! 3. require `INDEX.json` at the archive root
//! 4. atomically swap it in as `<app_base>/docs/{slug}`
//! 5. re-sync the catalog so the API reflects it immediately — no restart
//!
//! Nothing outside the staging directory is written until step 4 succeeds, and
//! step 4 rolls back if the swap fails, so the served documentation is never
//! left half-replaced.

use std::sync::Arc;

use rust_ef::db_context::DbContext;
use webx::*;

use docbit_contracts::docs::{
    IDocumentService, ResyncSummary, UploadWorkDocsRequest, UploadWorkDocsResponse,
};
use docbit_domain::new_id;

use crate::catalog;

use super::extract::{extract_zip, sanitize_slug, Staging};

#[derive(Inject)]
pub struct UploadWorkDocsHandler {
    #[inject(owned)]
    ctx: DbContext,
    #[inject]
    docs: Arc<dyn IDocumentService>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UploadWorkDocsRequest, UploadWorkDocsResponse> for UploadWorkDocsHandler {
    async fn handle(&mut self, req: UploadWorkDocsRequest) -> ActionResult<UploadWorkDocsResponse> {
        let slug = sanitize_slug(&req.slug)?;

        if req.archive.is_empty() {
            return Err(Error::validation("the archive is empty"));
        }

        // ── 解压到 staging（沙箱） ──
        let staging = Staging::create(&format!("docs-{}-{}", slug, new_id()))?;
        let scratch = Staging::create(&format!("zip-{}-{}", slug, new_id()))?;
        let archive_path = scratch.path().join("bundle.zip");

        req.archive
            .save_as(&archive_path)
            .await
            .map_err(|e| Error::internal(format!("Cannot store uploaded archive: {}", e)))?;

        let report = extract_zip(&archive_path, staging.path())?;

        if !staging.path().join("INDEX.json").is_file() {
            return Err(Error::validation(
                "the archive root must contain INDEX.json (zip the work's docs directory, \\\n                 not its parent)",
            ));
        }

        // ── 原子换入 ──
        let installed = self
            .docs
            .install_work_dir(&slug, staging.path())
            .map_err(Error::internal)?;
        // The staging tree has been renamed into place; nothing left to clean.
        staging.keep();

        // ── 目录同步（失败不回滚已安装的文件） ──
        let wwwroot = webx::app_base().join("wwwroot");
        let summary: ResyncSummary =
            catalog::resync_catalog(&mut self.ctx, self.docs.as_ref(), &wwwroot)
                .await
                .unwrap_or_else(|e| {
                    tracing::error!("[Docs] Catalog re-sync failed after install: {}", e);
                    ResyncSummary::default()
                });

        tracing::info!(
            "[Docs] Installed docs for '{}': {} files, {} bytes -> {}",
            slug,
            report.files,
            report.bytes,
            installed.display()
        );

        Ok(UploadWorkDocsResponse {
            slug,
            installed_dir: installed.to_string_lossy().into_owned(),
            files: report.files,
            bytes: report.bytes,
            catalog: summary,
        })
    }
}
