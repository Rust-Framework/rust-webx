//! Media storage — write an upload under `uploads/`, serve it back read-only.
//!
//! Two endpoints:
//!
//! | Endpoint | Role |
//! |----------|------|
//! | `POST /api/media` | store one uploaded file, return its URL + markdown |
//! | `GET /api/media/{*path}` | stream it back (inline for safe images, attachment otherwise) |
//!
//! The path a client may request is resolved strictly inside `uploads/`: any
//! `..`, empty or absolute segment is rejected before touching the filesystem.

use std::fs;
use std::path::{Component, Path, PathBuf};

use webx::*;

use docbit_contracts::media::{GetMediaRequest, MediaAsset, UploadMediaRequest};
use docbit_domain::new_id;

/// `uploads/` root — shared with the documentation upload staging area.
pub fn uploads_root() -> PathBuf {
    webx::app_base().join("uploads")
}

/// Extensions served inline with their real content type.
///
/// Raster formats only. SVG is excluded on purpose: an SVG served inline from
/// this origin can carry script, so it is offered as a download instead.
const INLINE_IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp"];

/// Extension for a stored file: the original's, lowercased and restricted to
/// `[a-z0-9]`, falling back to a mapping from the media type.
fn extension_for(original_name: &str, content_type: &str) -> String {
    let from_name = original_name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|ext| {
            !ext.is_empty() && ext.len() <= 10 && ext.chars().all(|c| c.is_ascii_alphanumeric())
        });

    from_name.unwrap_or_else(|| match content_type {
        "image/png" => "png".into(),
        "image/jpeg" => "jpg".into(),
        "image/gif" => "gif".into(),
        "image/webp" => "webp".into(),
        "image/avif" => "avif".into(),
        "image/svg+xml" => "svg".into(),
        "application/pdf" => "pdf".into(),
        "text/plain" => "txt".into(),
        "application/zip" => "zip".into(),
        _ => "bin".into(),
    })
}

/// Resolve a client-supplied relative path inside `uploads/`.
///
/// Returns `None` unless every component is a plain name — this is what keeps
/// `GET /api/media/../../etc/passwd` from resolving outside the root.
fn resolve_inside_uploads(relative: &str) -> Option<PathBuf> {
    if relative.is_empty() {
        return None;
    }
    let candidate = Path::new(relative);
    if candidate.is_absolute() {
        return None;
    }

    let mut safe = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Normal(name) => safe.push(name),
            // `..`, `.`, root and prefix are all rejected rather than normalised.
            _ => return None,
        }
    }
    if safe.as_os_str().is_empty() {
        return None;
    }
    Some(uploads_root().join(safe))
}

#[derive(Inject)]
pub struct UploadMediaHandler;

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UploadMediaRequest, MediaAsset> for UploadMediaHandler {
    async fn handle(&mut self, req: UploadMediaRequest) -> Result<MediaAsset> {
        if req.file.is_empty() {
            return Err(Error::Validation("no file was uploaded".into()));
        }

        let content_type = req
            .file
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_string();
        let is_image = content_type.starts_with("image/");
        let bucket = if is_image { "images" } else { "files" };

        let id = new_id();
        // Two hex characters of the id as a shard directory.
        let shard = &id[..2.min(id.len())];
        let name = req.file.file_name().to_string();
        let ext = extension_for(&name, &content_type);

        let relative = format!("{}/{}/{}.{}", bucket, shard, id, ext);
        let target = resolve_inside_uploads(&relative)
            .ok_or_else(|| Error::Internal("generated media path was rejected".into()))?;

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::Internal(format!("Cannot create media directory: {}", e)))?;
        }

        req.file
            .save_as(&target)
            .await
            .map_err(|e| Error::Internal(format!("Cannot store media file: {}", e)))?;

        let url = format!("/api/media/{}", relative);
        let markdown = if is_image {
            format!("![{}]({})", name, url)
        } else {
            format!("[{}]({})", name, url)
        };

        tracing::info!(
            "[Media] Stored {} ({} bytes) -> {}",
            name,
            req.file.size(),
            url
        );

        Ok(MediaAsset {
            url,
            name,
            content_type,
            size: req.file.size(),
            markdown,
        })
    }
}

#[derive(Inject)]
pub struct GetMediaHandler;

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetMediaRequest, ResponseData> for GetMediaHandler {
    async fn handle(&mut self, req: GetMediaRequest) -> Result<ResponseData> {
        let path = resolve_inside_uploads(&req.path)
            .ok_or_else(|| Error::Validation("invalid media path".into()))?;

        if !path.is_file() {
            return Err(Error::NotFound("Media not found".into()));
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        let response = ResponseData::file(path);
        Ok(if INLINE_IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            response
        } else {
            // Anything not on the inline allowlist is handed over as a download,
            // so a stored .svg/.html can never run in this origin.
            response.download_name(name)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{extension_for, resolve_inside_uploads};
    use std::path::Path;

    #[test]
    fn plain_relative_paths_resolve_under_the_uploads_root() {
        let resolved = resolve_inside_uploads("images/ab/x.png").expect("should resolve");
        assert!(resolved.ends_with("images/ab/x.png"));
    }

    #[test]
    fn traversal_is_rejected_individually() {
        for bad in [
            "../secrets",
            "images/../../etc/passwd",
            "/etc/passwd",
            "",
            ".",
            "..",
            "images/..",
        ] {
            assert!(
                resolve_inside_uploads(bad).is_none(),
                "'{bad}' must not resolve"
            );
        }
    }

    #[test]
    fn redundant_separators_are_normalised_not_traversed() {
        // `Path` folds `//` and interior `.` away, which cannot escape the root,
        // so these stay accepted — and, crucially, still land under `uploads/`.
        for benign in ["images//x.png", "images/./x.png"] {
            let resolved = resolve_inside_uploads(benign)
                .unwrap_or_else(|| panic!("'{benign}' should resolve"));
            assert!(
                resolved.ends_with(Path::new("images").join("x.png")),
                "'{benign}' resolved outside the expected shard: {}",
                resolved.display()
            );
        }
    }

    #[test]
    fn extension_comes_from_the_name_or_the_media_type() {
        assert_eq!(extension_for("Photo.PNG", "image/png"), "png");
        assert_eq!(extension_for("no-extension", "image/webp"), "webp");
        assert_eq!(extension_for("weird.@@@", "application/pdf"), "pdf");
        assert_eq!(extension_for("archive.zip", "application/zip"), "zip");
        // A path-ish name must not smuggle separators into the stored name.
        assert_eq!(extension_for("a.b/c", "application/octet-stream"), "bin");
    }
}
