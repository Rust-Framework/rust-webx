//! Documentation bundle upload 鈥?accept a zip, install it, re-sync the catalog.
//!
//! Flow (`POST /api/works/{slug}/docs`, admin only):
//!
//! 1. spool the uploaded archive to a temporary file
//! 2. extract it into a staging directory, sandboxed (see [`extract_zip`])
//! 3. require `INDEX.json` at the archive root
//! 4. atomically swap it in as `<app_base>/docs/{slug}`
//! 5. re-sync the catalog so the API reflects it immediately 鈥?no restart
//!
//! Nothing outside the staging directory is written until step 4 succeeds, and
//! step 4 rolls back if the swap fails, so the served documentation is never
//! left half-replaced.

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rust_ef::db_context::DbContext;
use webx::*;

use docbit_contracts::docs::{
    IDocumentService, ResyncSummary, UploadWorkDocsRequest, UploadWorkDocsResponse,
};
use docbit_domain::new_id;

use crate::catalog;

/// Upper bound on archive entries 鈥?a docs bundle is hundreds of files, not
/// hundreds of thousands.
const MAX_ENTRIES: usize = 20_000;
/// Upper bound on total uncompressed bytes (zip-bomb guard).
const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
/// Upper bound on any single uncompressed file.
const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;

/// Where uploads are staged. Kept outside `docs/` so a partial upload can never
/// be mistaken for a real work directory.
pub fn uploads_root() -> PathBuf {
    webx::app_base().join("uploads")
}

/// Removes the staging directory unless it is explicitly kept.
struct Staging {
    path: PathBuf,
    keep: bool,
}

impl Staging {
    fn create(name: &str) -> Result<Self> {
        let path = uploads_root().join(".tmp").join(name);
        fs::create_dir_all(&path)
            .map_err(|e| Error::Internal(format!("Cannot create staging dir: {}", e)))?;
        Ok(Self { path, keep: false })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// What one extraction produced.
#[derive(Debug, Clone, Copy)]
pub struct ExtractReport {
    pub files: usize,
    pub bytes: u64,
}

/// A slug may only contain lowercase letters, digits and hyphens.
///
/// This is the boundary that keeps an upload from naming a directory outside
/// `docs/` 鈥?`../`, absolute paths and Windows separators are all rejected here
/// rather than being sanitised into something unexpected.
fn sanitize_slug(raw: &str) -> Result<String> {
    if raw.is_empty() || raw.len() > 64 {
        return Err(Error::Validation("slug must be 1-64 characters".into()));
    }
    let ok = raw
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !ok || raw.starts_with('-') || raw.ends_with('-') {
        return Err(Error::Validation(format!(
            "invalid slug '{raw}': use lowercase letters, digits and inner hyphens"
        )));
    }
    Ok(raw.to_string())
}

/// Extract `archive` into `dest`, rejecting anything that would escape it.
///
/// Rejects absolute paths, `..` segments, backslashes (a separator on Windows),
/// `:` (drive letters and NTFS alternate data streams) and symlink entries. Each
/// file is copied with a hard byte cap, and the whole archive is capped too, so a
/// zip bomb cannot fill the disk.
pub fn extract_zip(archive: &Path, dest: &Path) -> Result<ExtractReport> {
    let file = fs::File::open(archive)
        .map_err(|e| Error::Validation(format!("Cannot read uploaded archive: {}", e)))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| Error::Validation(format!("Not a valid zip archive: {}", e)))?;

    if zip.len() > MAX_ENTRIES {
        return Err(Error::Validation(format!(
            "archive has {} entries; the limit is {MAX_ENTRIES}",
            zip.len()
        )));
    }

    let mut files = 0usize;
    let mut bytes = 0u64;

    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| Error::Validation(format!("Corrupt archive entry {index}: {e}")))?;

        let raw_name = entry.name().to_string();
        if raw_name.contains('\\') || raw_name.contains(':') {
            return Err(Error::Validation(format!(
                "archive entry '{raw_name}' is not a plain relative path"
            )));
        }

        // Symlinks would let the archive point outside the install directory.
        if let Some(mode) = entry.unix_mode() {
            if mode & 0o170_000 == 0o120_000 {
                return Err(Error::Validation(format!(
                    "archive entry '{raw_name}' is a symlink"
                )));
            }
        }

        // `enclosed_name` returns None for absolute paths and `..` escapes.
        let Some(relative) = entry.enclosed_name() else {
            return Err(Error::Validation(format!(
                "archive entry '{raw_name}' escapes the archive root"
            )));
        };
        if relative.as_os_str().is_empty() {
            continue;
        }

        let target = dest.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|e| {
                Error::Internal(format!("Cannot create '{}': {}", target.display(), e))
            })?;
            continue;
        }

        if entry.size() > MAX_FILE_BYTES {
            return Err(Error::Validation(format!(
                "'{raw_name}' is {} MiB; the per-file limit is {} MiB",
                entry.size() / (1024 * 1024),
                MAX_FILE_BYTES / (1024 * 1024)
            )));
        }
        if bytes + entry.size() > MAX_TOTAL_BYTES {
            return Err(Error::Validation(format!(
                "archive expands beyond the {} MiB limit",
                MAX_TOTAL_BYTES / (1024 * 1024)
            )));
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                Error::Internal(format!("Cannot create '{}': {}", parent.display(), e))
            })?;
        }

        let mut out = fs::File::create(&target)
            .map_err(|e| Error::Internal(format!("Cannot write '{}': {}", target.display(), e)))?;

        // `take` enforces the per-file cap even if the header lies.
        let written = std::io::copy(&mut entry.by_ref().take(MAX_FILE_BYTES + 1), &mut out)
            .map_err(|e| Error::Internal(format!("Cannot write '{}': {}", target.display(), e)))?;
        if written > MAX_FILE_BYTES {
            return Err(Error::Validation(format!(
                "'{raw_name}' exceeds the {} MiB per-file limit",
                MAX_FILE_BYTES / (1024 * 1024)
            )));
        }

        files += 1;
        bytes += written;
    }

    Ok(ExtractReport { files, bytes })
}

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
    async fn handle(&mut self, req: UploadWorkDocsRequest) -> Result<UploadWorkDocsResponse> {
        let slug = sanitize_slug(&req.slug)?;

        if req.archive.is_empty() {
            return Err(Error::Validation("the archive is empty".into()));
        }

        let staging = Staging::create(&format!("docs-{}-{}", slug, new_id()))?;
        let scratch = Staging::create(&format!("zip-{}-{}", slug, new_id()))?;
        let archive_path = scratch.path().join("bundle.zip");

        req.archive
            .save_as(&archive_path)
            .await
            .map_err(|e| Error::Internal(format!("Cannot store uploaded archive: {}", e)))?;

        let report = extract_zip(&archive_path, staging.path())?;

        if !staging.path().join("INDEX.json").is_file() {
            return Err(Error::Validation(
                "the archive root must contain INDEX.json (zip the work's docs directory, \
                 not its parent)"
                    .into(),
            ));
        }

        let installed = self
            .docs
            .install_work_dir(&slug, staging.path())
            .map_err(Error::Internal)?;
        // The staging tree has been renamed into place; nothing left to clean.
        staging.keep();

        let wwwroot = webx::app_base().join("wwwroot");
        let summary: ResyncSummary =
            catalog::resync_catalog(&mut self.ctx, self.docs.as_ref(), &wwwroot)
                .await
                .unwrap_or_else(|e| {
                    // The files are installed and served from disk either way; a
                    // failed re-sync is reported without discarding the upload.
                    tracing::error!("[Works] Catalog re-sync failed after install: {}", e);
                    ResyncSummary::default()
                });

        tracing::info!(
            "[Works] Installed docs for '{}': {} files, {} bytes -> {}",
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

#[cfg(test)]
mod tests {
    use super::{extract_zip, sanitize_slug};
    use std::fs;
    use std::io::Write as _;
    use std::path::Path;

    /// Minimal zip writer so a test can state an entry name *and* its unix mode.
    fn write_zip(path: &Path, entries: &[(&str, &[u8], u32)]) {
        let file = fs::File::create(path).expect("create zip");
        let mut writer = zip::ZipWriter::new(file);
        for (name, bytes, mode) in entries {
            let options = zip::write::SimpleFileOptions::default().unix_permissions(*mode);
            writer.start_file(*name, options).expect("start entry");
            writer.write_all(bytes).expect("write entry");
        }
        writer.finish().expect("finish zip");
    }

    /// A zip containing one symlink entry.
    ///
    /// `unix_permissions` cannot express a symlink — it always records the
    /// entry as a regular file — so the dedicated writer API is required.
    fn write_symlink_zip(path: &Path, name: &str, target: &str) {
        let file = fs::File::create(path).expect("create zip");
        let mut writer = zip::ZipWriter::new(file);
        writer
            .add_symlink(name, target, zip::write::SimpleFileOptions::default())
            .expect("add symlink entry");
        writer.finish().expect("finish zip");
    }

    fn staging() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn a_plain_bundle_extracts() {
        let dir = staging();
        let archive = dir.path().join("docs.zip");
        write_zip(
            &archive,
            &[
                ("INDEX.json", b"{}", 0o644),
                ("ch/INDEX.md", b"# hi", 0o644),
            ],
        );

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();

        let report = extract_zip(&archive, &dest).expect("should extract");
        assert_eq!(report.files, 2);
        assert!(dest.join("INDEX.json").is_file());
        assert!(dest.join("ch/INDEX.md").is_file());
    }

    #[test]
    fn a_traversing_entry_is_rejected() {
        let dir = staging();
        let archive = dir.path().join("evil.zip");
        write_zip(&archive, &[("../escaped.txt", b"x", 0o644)]);

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();

        let err = extract_zip(&archive, &dest).expect_err("must reject traversal");
        assert!(err.to_string().contains("escapes"), "unexpected: {err}");
        assert!(
            !dir.path().join("escaped.txt").exists(),
            "nothing may be written outside the destination"
        );
    }

    #[test]
    fn a_symlink_entry_is_rejected() {
        let dir = staging();
        let archive = dir.path().join("link.zip");
        write_symlink_zip(&archive, "link", "/etc/passwd");

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();

        let err = extract_zip(&archive, &dest).expect_err("must reject symlinks");
        assert!(err.to_string().contains("symlink"), "unexpected: {err}");
    }

    #[test]
    fn a_backslash_entry_is_rejected() {
        let dir = staging();
        let archive = dir.path().join("win.zip");
        write_zip(&archive, &[("..\\escaped.txt", b"x", 0o644)]);

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();

        let err = extract_zip(&archive, &dest).expect_err("must reject backslashes");
        assert!(
            err.to_string().contains("relative path"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn a_non_zip_upload_is_a_validation_error() {
        let dir = staging();
        let archive = dir.path().join("not-a-zip.zip");
        fs::write(&archive, b"definitely not a zip").unwrap();

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();

        let err = extract_zip(&archive, &dest).expect_err("must reject");
        assert!(err.to_string().contains("valid zip"), "unexpected: {err}");
    }

    #[test]
    fn slugs_may_only_be_lowercase_alphanumeric_with_inner_hyphens() {
        assert_eq!(sanitize_slug("rust-webx").unwrap(), "rust-webx");
        assert_eq!(sanitize_slug("a1").unwrap(), "a1");

        for bad in [
            "",
            "..",
            "../etc",
            "a/b",
            "a\\b",
            "Rust-Webx",
            "rust webx",
            "-lead",
            "trail-",
            "a.b",
        ] {
            assert!(
                sanitize_slug(bad).is_err(),
                "'{bad}' must not be accepted as a slug"
            );
        }
        assert!(
            sanitize_slug(&"a".repeat(65)).is_err(),
            "length must be capped"
        );
    }
}
