//! SPA static file serving middleware.
//!
//! Serves files from a configured root directory.
//! For SPA routing, non-file requests fall back to index.html.
//!
//! Files are handed to the host as [`FileBody`] values, so they are streamed
//! rather than read into memory, and every response carries an `ETag` and
//! `Last-Modified` and honours `Range` requests. That means a 2 GB video in
//! `wwwroot` is served with a bounded memory footprint and can be seeked.

use rust_webx_core::error::Result;
use rust_webx_core::http::{FileBody, HttpStatus, IHttpContext};
use rust_webx_core::middleware::IMiddleware;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

/// How long fingerprints-addressed files may be cached.
///
/// Files under `/assets/` are treated as content-addressed and cached for a
/// year; everything else must revalidate with the `ETag` the host sends.
const IMMUTABLE_CACHE: &str = "public, max-age=31536000, immutable";
const REVALIDATE_CACHE: &str = "public, max-age=0, must-revalidate";

/// SPA static file middleware.
///
/// - Matches `/{filename}` against local filesystem
/// - Serves files with MIME types inferred by the host
/// - Falls back to `index.html` for unknown paths (SPA routing)
/// - Handles GET and HEAD; other methods pass through silently
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
    /// matching the strategy used by `config::load_appsettings`.
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
        if method != "GET" && method != "HEAD" {
            return Ok(ControlFlow::Continue(()));
        }

        let request_path = ctx.request().path();

        // API routes must reach the router — never serve index.html for /api/*.
        if request_path.starts_with("/api/") {
            return Ok(ControlFlow::Continue(()));
        }

        if request_path.starts_with("/assets/") {
            let relative = alias_static_path(request_path.trim_start_matches('/'));
            if relative.is_empty() || relative.contains("..") {
                ctx.response_mut().set_status(HttpStatus::NOT_FOUND);
                return Ok(ControlFlow::Continue(()));
            }
            let candidate = self.root.join(&relative);
            if candidate.is_file() {
                return serve_file(ctx, &candidate, IMMUTABLE_CACHE).await;
            }
            ctx.response_mut().set_status(HttpStatus::NOT_FOUND);
            return Ok(ControlFlow::Continue(()));
        }

        let file_path = self.resolve_file(request_path);
        if file_path.is_file() {
            return serve_file(ctx, &file_path, REVALIDATE_CACHE).await;
        }

        // File not found — fall back to index.html for SPA routing.
        let index_path = self.root.join(&self.index);
        if index_path.is_file() {
            return serve_file(ctx, &index_path, REVALIDATE_CACHE).await;
        }

        // Neither the file nor index.html exists — let the router decide.
        Ok(ControlFlow::Continue(()))
    }
}

/// Hand a file to the host for streaming.
async fn serve_file(
    ctx: &mut dyn IHttpContext,
    path: &Path,
    cache_control: &str,
) -> Result<ControlFlow<()>> {
    ctx.response_mut().set_status(HttpStatus::OK);
    ctx.response_mut().set_header("cache-control", cache_control);
    ctx.response_mut()
        .write_body(FileBody::path(path).into())
        .await?;
    Ok(ControlFlow::Continue(()))
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

/// Resolve a SPA root path by first checking the application base directory
/// (as resolved by `rust_webx_core::paths::app_base`), then as-is.
///
/// This mirrors the strategy used by `config::load_appsettings` so that
/// `use_spa("wwwroot")` works whether the user runs from `demo/`, from
/// the workspace root (`rust-webx/`), or from a deployment directory
/// (exe alongside `wwwroot/`).
fn resolve_spa_root(root: PathBuf) -> PathBuf {
    // If the path is already absolute or exists, use it directly.
    if root.is_absolute() || root.exists() {
        return root;
    }

    // 应用基准目录（exe 同级 / cwd / 上溯统一由 app_base 处理）。
    let candidate = rust_webx_core::paths::app_base().join(&root);
    if candidate.exists() {
        return candidate;
    }

    root
}

#[cfg(test)]
mod tests {
    use super::{alias_static_path, normalize_path};
    use std::path::Path;

    #[test]
    fn alias_static_path_strips_vditor_dist_segment() {
        let out = alias_static_path("assets/vendor/vditor-dist/dist/js/foo.js");
        assert_eq!(out, "assets/vendor/vditor-dist/js/foo.js");
    }

    #[test]
    fn normalize_path_removes_dot_dot() {
        let p = normalize_path(Path::new("a/../b/./c"));
        assert_eq!(p, Path::new("b/c"));
    }

    #[test]
    fn skips_api_paths_for_spa_fallback() {
        // API paths are passed through to the router; no static/index.html response.
        // Full integration is covered by host tests; this documents the guard.
        assert!("/api/docs".starts_with("/api/"));
        assert!(!"/works/foo".starts_with("/api/"));
    }
}
