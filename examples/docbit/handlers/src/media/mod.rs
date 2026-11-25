//! Media context — image/attachment upload and public serving.
//!
//! Files live under `<app_base>/uploads/` (see [`crate::common::uploads_root`]),
//! deliberately outside the embedded `wwwroot`, and are served through
//! `/api/media/...` so the SPA middleware only ever serves the frontend.

pub mod get_media;
pub mod upload_media;

pub use get_media::GetMediaHandler;
pub use upload_media::UploadMediaHandler;

use std::path::{Component, Path, PathBuf};

/// Extensions served inline with their real content type.
pub(crate) const INLINE_IMAGE_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp"];

/// Extension for a stored file: the original's, lowercased and restricted to
/// `[a-z0-9]`, falling back to a mapping from the media type.
pub(crate) fn extension_for(original_name: &str, content_type: &str) -> String {
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
pub(crate) fn resolve_inside_uploads(relative: &str) -> Option<PathBuf> {
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
    Some(crate::common::uploads_root().join(safe))
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
