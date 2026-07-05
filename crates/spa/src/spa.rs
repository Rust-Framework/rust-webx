//! SPA static file serving middleware.
//!
//! Serves files from a configured root directory.
//! For SPA routing, non-file requests fall back to index.html.

use rust_webapp_core::error::Result;
use rust_webapp_core::http::{HttpStatus, IHttpContext};
use rust_webapp_core::middleware::IMiddleware;
use std::ops::ControlFlow;
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
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        let method = ctx.request().method().to_uppercase();
        if method != "GET" {
            return Ok(ControlFlow::Continue(()));
        }

        let request_path = ctx.request().path();

        if request_path.starts_with("/assets/") {
            let relative = alias_static_path(request_path.trim_start_matches('/'));
            if relative.is_empty() {
                ctx.response_mut().set_status(HttpStatus::NOT_FOUND);
                return Ok(ControlFlow::Continue(()));
            }
            let candidate = self.root.join(&relative);
            if relative.contains("..") || !candidate.is_file() {
                ctx.response_mut().set_status(HttpStatus::NOT_FOUND);
                return Ok(ControlFlow::Continue(()));
            }
            match tokio::fs::read(&candidate).await {
                Ok(data) => {
                    ctx.response_mut().set_status(HttpStatus::OK);
                    ctx.response_mut()
                        .set_header("content-type", mime_type(&candidate));
                    ctx.response_mut().write_bytes(data).await?;
                }
                Err(_) => {
                    ctx.response_mut().set_status(HttpStatus::NOT_FOUND);
                }
            }
            return Ok(ControlFlow::Continue(()));
        }

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

        Ok(ControlFlow::Continue(()))
    }
}

impl SpaMiddleware {
    /// Resolve a request path to a filesystem path, preventing traversal.
    fn resolve_file(&self, request_path: &str) -> PathBuf {
        let relative = alias_static_path(request_path.trim_start_matches('/'));
        if relative.is_empty() {
            return self.root.join(&self.index);
        }

        let candidate = self.root.join(&relative);

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

/// Map vendor paths that include a redundant `dist/` segment to on-disk layout.
///
/// Vditor resolves assets as `{cdn}/dist/js/...` while our vendored tree keeps
/// `js/`, `css/`, and bundle files at the package root.
fn alias_static_path(relative: &str) -> String {
    const VDITOR_DIST: &str = "assets/vendor/vditor-dist/dist/";
    if let Some(rest) = relative.strip_prefix(VDITOR_DIST) {
        format!("assets/vendor/vditor-dist/{rest}")
    } else {
        relative.to_string()
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

/// Resolve a SPA root path by first checking the application base directory
/// (as resolved by `rust_webapp_core::paths::app_base`), then as-is.
///
/// This mirrors the strategy used by `config::load_appsettings` so that
/// `use_spa("wwwroot")` works whether the user runs from `demo/`, from
/// the workspace root (`rust-webapp/`), or from a deployment directory
/// (exe alongside `wwwroot/`).
fn resolve_spa_root(root: PathBuf) -> PathBuf {
    // If the path is already absolute or exists, use it directly.
    if root.is_absolute() || root.exists() {
        return root;
    }

    // 应用基准目录（exe 同级 / cwd / 上溯统一由 app_base 处理）。
    let candidate = rust_webapp_core::paths::app_base().join(&root);
    if candidate.exists() {
        return candidate;
    }

    root
}
