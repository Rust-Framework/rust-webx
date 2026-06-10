//! SPA static file serving middleware.
//!
//! Serves files from a configured root directory.
//! For SPA routing, non-file requests fall back to index.html.

use lrwf_core::error::Result;
use lrwf_core::http::{HttpStatus, IHttpContext};
use lrwf_core::middleware::IMiddleware;
use std::path::{Path, PathBuf};

/// SPA static file middleware.
///
/// - Matches `/{filename}` against local filesystem
/// - Serves files with auto-detected MIME types
/// - Falls back to `index.html` for unknown paths (SPA routing)
/// - Only handles GET requests; non-GET passes through silently
pub struct SpaMiddleware {
    root: PathBuf,
    index: String,
}

impl SpaMiddleware {
    /// Create a new SPA middleware with default index "index.html".
    ///
    /// The `root` path is resolved relative to the current working directory.
    /// If the directory doesn't exist at that path, the middleware searches
    /// upward through ancestor directories and their immediate subdirectories,
    /// matching the strategy used by [`config::load_appsettings`].
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: resolve_spa_root(root.into()),
            index: "index.html".to_string(),
        }
    }

    /// Create a new SPA middleware with a custom index file name.
    pub fn with_index(root: impl Into<PathBuf>, index: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            index: index.into(),
        }
    }
}

#[async_trait::async_trait]
impl IMiddleware for SpaMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let method = ctx.request().method().to_uppercase();
        if method != "GET" {
            return Ok(());
        }

        let request_path = ctx.request().path();
        let file_path = self.resolve_file(request_path);

        match tokio::fs::read(&file_path).await {
            Ok(data) => {
                ctx.response_mut().set_status(HttpStatus::OK);
                ctx.response_mut()
                    .set_header("content-type", mime_type(&file_path));
                ctx.response_mut().write_bytes(data).await?;
            }
            Err(_) => {
                // File not found — try fallback to index.html for SPA routing
                let index_path = self.root.join(&self.index);
                match tokio::fs::read(&index_path).await {
                    Ok(data) => {
                        ctx.response_mut().set_status(HttpStatus::OK);
                        ctx.response_mut().set_header("content-type", "text/html");
                        ctx.response_mut().write_bytes(data).await?;
                    }
                    Err(_) => {
                        // Neither file nor index.html exists — pass through
                    }
                }
            }
        }

        Ok(())
    }
}

impl SpaMiddleware {
    /// Resolve a request path to a filesystem path, preventing traversal.
    fn resolve_file(&self, request_path: &str) -> PathBuf {
        let relative = request_path.trim_start_matches('/');
        if relative.is_empty() {
            return self.root.join(&self.index);
        }

        let candidate = self.root.join(relative);

        // Canonicalize to detect and prevent path traversal attacks.
        // If canonicalization fails (file doesn't exist), do a manual
        // check against the configured root.
        match candidate.canonicalize() {
            Ok(resolved) => {
                let root_canonical = self
                    .root
                    .canonicalize()
                    .unwrap_or_else(|_| self.root.clone());
                if resolved.starts_with(&root_canonical) {
                    resolved
                } else {
                    // Path traversal attempt — fall back to index.html
                    self.root.join(&self.index)
                }
            }
            Err(_) => {
                // File doesn't exist; do a simple traversal check on the
                // unresolved path before returning it for the caller to try.
                if is_safe_subpath(&self.root, &candidate) {
                    candidate
                } else {
                    self.root.join(&self.index)
                }
            }
        }
    }
}

/// Check that `candidate` is a sub-path of `root` without requiring the
/// candidate to exist on disk (canonicalize requires the file exists).
fn is_safe_subpath(root: &Path, candidate: &Path) -> bool {
    // Normalize both paths by stripping "." and ".." components.
    let normalized = normalize_path(candidate);
    let root_abs = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    // If the candidate is absolute, check it starts with the absolute root.
    if normalized.is_absolute() {
        return normalized.starts_with(&root_abs);
    }

    // For relative candidates, resolve against the current dir and compare.
    if let Ok(cwd) = std::env::current_dir() {
        let abs_candidate = cwd.join(&normalized);
        if let Ok(canon) = abs_candidate.canonicalize() {
            return canon.starts_with(&root_abs);
        }
    }

    true
}

/// Remove "." and ".." segments from a path without consulting the filesystem.
fn normalize_path(path: &Path) -> PathBuf {
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::CurDir => {}
            other => {
                parts.push(other.as_os_str());
            }
        }
    }
    let mut result = PathBuf::new();
    for part in parts {
        result.push(part);
    }
    result
}

/// Detect MIME type from file extension.
fn mime_type(path: &Path) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "html" | "htm" => "text/html",
        "js" | "mjs" => "application/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "eot" => "application/vnd.ms-fontobject",
        "txt" => "text/plain",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// Resolve a SPA root path by first checking as-is, then walking up
/// from cwd and checking each ancestor's immediate subdirectories.
///
/// This mirrors the strategy used by `config::load_appsettings` so that
/// `use_spa("wwwroot")` works whether the user runs from `demo/` or from
/// the workspace root (`lrwf/`).
fn resolve_spa_root(root: PathBuf) -> PathBuf {
    // If the path is already absolute or exists, use it directly.
    if root.is_absolute() || root.exists() {
        return root;
    }

    // Walk up from cwd; at each ancestor check immediate subdirectories.
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir = Some(cwd.as_path());
        while let Some(d) = dir {
            if let Ok(entries) = std::fs::read_dir(d) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        let candidate = entry.path().join(&root);
                        if candidate.exists() {
                            return candidate;
                        }
                    }
                }
            }
            dir = d.parent();
        }
    }

    root
}
