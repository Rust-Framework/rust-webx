//! SPA static file serving middleware.
//!
//! Serves files from a configured disk directory, from a table of files
//! compiled into the executable, or both — in which case the disk copy of a
//! given file wins and the embedded copy is the fallback.
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

use crate::embed::{EmbeddedAsset, EmbeddedAssets};

/// How long fingerprints-addressed files may be cached.
///
/// Files under `/assets/` are treated as content-addressed and cached for a
/// year; everything else must revalidate with the `ETag` the host sends.
const IMMUTABLE_CACHE: &str = "public, max-age=31536000, immutable";
const REVALIDATE_CACHE: &str = "public, max-age=0, must-revalidate";

/// Environment variable that turns embedded assets off at runtime.
///
/// Set it to `off` (or `0` / `false` / `no`) to ignore the compiled-in files
/// and serve only from disk — handy for bisecting an override problem without
/// rebuilding.
pub const EMBED_ENV: &str = "RUST_WEBX_EMBED";

/// Where a [`SpaMiddleware`] takes its files from.
///
/// Built by `Host::builder()`: `use_spa(...)` names the disk directory and
/// `embed(...)` adds compiled-in files underneath it.
#[derive(Debug, Clone)]
pub enum SpaSource {
    /// A directory, resolved the same way as `config::load_appsettings` does.
    Disk(String),
    /// Files compiled into the executable, served for any name that is not
    /// present under `overlay_root`.
    Embedded {
        /// The compiled-in baseline.
        assets: EmbeddedAssets,
        /// The disk directory consulted first.
        overlay_root: String,
    },
}

impl SpaSource {
    /// The disk directory consulted first; it may not exist.
    pub fn disk_root(&self) -> &str {
        match self {
            SpaSource::Disk(root) => root,
            SpaSource::Embedded { overlay_root, .. } => overlay_root,
        }
    }

    /// The embedded baseline, if this source has one.
    pub fn embedded(&self) -> Option<&EmbeddedAssets> {
        match self {
            SpaSource::Disk(_) => None,
            SpaSource::Embedded { assets, .. } => Some(assets),
        }
    }

    /// Use `dir` on disk, replacing whatever root was configured before.
    ///
    /// This is how `use_spa("wwwroot")` before `embed(...)` states where
    /// operators may drop overrides, independently of the directory the assets
    /// were compiled from.
    pub fn with_overlay_root(self, dir: impl Into<String>) -> Self {
        let dir = dir.into();
        match self {
            SpaSource::Disk(_) => SpaSource::Disk(dir),
            SpaSource::Embedded { assets, .. } => SpaSource::Embedded {
                assets,
                overlay_root: dir,
            },
        }
    }
}

impl From<&str> for SpaSource {
    fn from(root: &str) -> Self {
        SpaSource::Disk(root.to_string())
    }
}

impl From<String> for SpaSource {
    fn from(root: String) -> Self {
        SpaSource::Disk(root)
    }
}

impl From<PathBuf> for SpaSource {
    fn from(root: PathBuf) -> Self {
        SpaSource::Disk(root.to_string_lossy().into_owned())
    }
}

impl From<&Path> for SpaSource {
    fn from(root: &Path) -> Self {
        SpaSource::Disk(root.to_string_lossy().into_owned())
    }
}

impl From<&String> for SpaSource {
    fn from(root: &String) -> Self {
        SpaSource::Disk(root.clone())
    }
}

impl From<EmbeddedAssets> for SpaSource {
    fn from(assets: EmbeddedAssets) -> Self {
        SpaSource::Embedded {
            overlay_root: assets.root().to_string(),
            assets,
        }
    }
}

/// SPA static file middleware.
///
/// - Matches `/{filename}` against the disk root, then against embedded files
/// - Serves files with MIME types inferred by the host
/// - Falls back to `index.html` for unknown paths (SPA routing)
/// - Handles GET and HEAD; other methods pass through silently
pub struct SpaMiddleware {
    root: PathBuf,
    index: String,
    embedded: Option<EmbeddedAssets>,
    /// Embedded files are served only when this is on; see [`EMBED_ENV`].
    embed_enabled: bool,
}

impl SpaMiddleware {
    /// Create a new SPA middleware with default index "index.html".
    ///
    /// The disk root is resolved relative to the current working directory.
    /// If the directory doesn't exist at that path, the middleware searches
    /// upward through ancestor directories and their immediate subdirectories,
    /// matching the strategy used by `config::load_appsettings`.
    pub fn new(source: impl Into<SpaSource>) -> Self {
        Self::build(source.into(), "index.html".to_string())
    }

    /// Create a new SPA middleware with a custom index file name.
    pub fn with_index(source: impl Into<SpaSource>, index: impl Into<String>) -> Self {
        Self::build(source.into(), index.into())
    }

    /// Create a SPA middleware from an already-built source.
    pub fn from_source(source: SpaSource) -> Self {
        Self::build(source, "index.html".to_string())
    }

    fn build(source: SpaSource, index: String) -> Self {
        let embedded = source.embedded().copied();
        if let Some(assets) = embedded {
            assets.debug_assert_sorted();
        }
        Self {
            root: resolve_spa_root(PathBuf::from(source.disk_root())),
            index,
            embedded,
            embed_enabled: embedded.is_some() && embed_enabled_from_env(),
        }
    }

    /// The disk directory actually consulted, after resolution.
    pub fn resolved_root(&self) -> &Path {
        &self.root
    }

    /// The embedded files this middleware may serve, if any.
    ///
    /// Present even when [`EMBED_ENV`] disabled them at runtime.
    pub fn embedded(&self) -> Option<&EmbeddedAssets> {
        self.embedded.as_ref()
    }

    /// The embedded file for a normalized key, when embedding is active.
    fn embedded_asset(&self, key: &str) -> Option<&'static EmbeddedAsset> {
        if !self.embed_enabled {
            return None;
        }
        self.embedded.as_ref()?.get(key)
    }

    /// Normalize a request path into an embedded-table key.
    ///
    /// Returns `None` for paths that try to leave the root; the embedded table
    /// is an exact-match set, so those must never be resolved loosely.
    fn embedded_key(&self, request_path: &str) -> Option<String> {
        let relative = alias_static_path(request_path.trim_start_matches('/'));
        if relative.contains("..") {
            return None;
        }
        Some(if relative.is_empty() {
            self.index.clone()
        } else {
            relative
        })
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
            if let Some(asset) = self.embedded_asset(&relative) {
                return serve_embedded(ctx, asset, IMMUTABLE_CACHE).await;
            }
            ctx.response_mut().set_status(HttpStatus::NOT_FOUND);
            return Ok(ControlFlow::Continue(()));
        }

        // A file of this exact name: the disk copy wins, the embedded copy is
        // the fallback. A request for "/" resolves to the index file, so
        // "index.html" gets the same precedence.
        if let Some(file_path) = self.resolve_file(request_path) {
            if file_path.is_file() {
                return serve_file(ctx, &file_path, REVALIDATE_CACHE).await;
            }
        }
        if let Some(key) = self.embedded_key(request_path) {
            if let Some(asset) = self.embedded_asset(&key) {
                return serve_embedded(ctx, asset, REVALIDATE_CACHE).await;
            }
        }

        // File not found — fall back to index.html for SPA routing.
        let index_path = self.root.join(&self.index);
        if index_path.is_file() {
            return serve_file(ctx, &index_path, REVALIDATE_CACHE).await;
        }
        if let Some(asset) = self.embedded_asset(&self.index) {
            return serve_embedded(ctx, asset, REVALIDATE_CACHE).await;
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
    ctx.response_mut()
        .set_header("cache-control", cache_control);
    ctx.response_mut()
        .write_body(FileBody::path(path).into())
        .await?;
    Ok(ControlFlow::Continue(()))
}

/// Hand a compiled-in file to the host.
///
/// The source is seekable and its length is known, so the response carries an
/// exact `Content-Length` and supports `Range`, `If-None-Match` and `HEAD` just
/// like a file on disk. The `ETag` comes from the table because there is no
/// mtime to derive one from.
async fn serve_embedded(
    ctx: &mut dyn IHttpContext,
    asset: &'static EmbeddedAsset,
    cache_control: &str,
) -> Result<ControlFlow<()>> {
    ctx.response_mut().set_status(HttpStatus::OK);
    ctx.response_mut()
        .set_header("cache-control", cache_control);
    ctx.response_mut()
        .write_body(
            FileBody::static_bytes(asset.bytes)
                .content_type(asset.content_type)
                .entity_tag(asset.etag)
                .into(),
        )
        .await?;
    Ok(ControlFlow::Continue(()))
}

impl SpaMiddleware {
    /// Resolve a request path to a filesystem path, preventing traversal.
    /// Resolve a request path to the file it names under the root.
    ///
    /// `None` means the path escapes the root, so it is treated as a miss.
    /// A returned path may still not exist: the caller tries disk first, then
    /// the compiled-in copy, before falling back to the index for SPA routing.
    /// Resolving a *missing* file straight to the index here would hide every
    /// embedded file that has no disk counterpart, so this method never
    /// substitutes the index.
    fn resolve_file(&self, request_path: &str) -> Option<PathBuf> {
        let relative = alias_static_path(request_path.trim_start_matches('/'));
        if relative.is_empty() {
            return Some(self.root.join(&self.index));
        }

        let candidate = self.root.join(&relative);

        // Reject anything that escapes the root before touching the filesystem.
        if !is_safe_subpath(&self.root, &candidate) {
            return None;
        }

        // Follow symlinks only when the result still stays inside the root.
        match candidate.canonicalize() {
            Ok(resolved) if is_canonical_subpath(&self.root, &resolved) => Some(resolved),
            Ok(_) => None,
            Err(_) => Some(candidate),
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

/// Whether embedded files should be served, per [`EMBED_ENV`].
fn embed_enabled_from_env() -> bool {
    embed_enabled(std::env::var(EMBED_ENV).ok().as_deref())
}

/// Interpret the [`EMBED_ENV`] value. Unset, or anything not recognised as
/// "off", leaves embedding enabled.
fn embed_enabled(value: Option<&str>) -> bool {
    match value {
        Some(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "off" | "0" | "false" | "no"
        ),
        None => true,
    }
}

/// Check that `candidate` is a sub-path of `root` without requiring the
/// candidate to exist on disk (canonicalize requires the file exists).
/// Whether `candidate` stays inside `root`, compared lexically.
///
/// Deliberately avoids `canonicalize` on the candidate: on Windows it returns a
/// `\\?\`-prefixed verbatim path, which never compares equal to a plainly
/// joined one, so every missing file would look like a traversal attempt.
/// `..` segments are removed without consulting the filesystem, which is what
/// the traversal defence actually needs.
fn is_safe_subpath(root: &Path, candidate: &Path) -> bool {
    let root_abs = normalize_path(&absolute(root));
    let candidate_abs = normalize_path(&absolute(candidate));
    candidate_abs.starts_with(&root_abs)
}

/// Whether an already-canonicalized path stays inside the root, following
/// symlinks on both sides.
fn is_canonical_subpath(root: &Path, resolved: &Path) -> bool {
    match root.canonicalize() {
        Ok(root) => resolved.starts_with(&root),
        // The root itself is missing, so there is nothing to compare against.
        Err(_) => is_safe_subpath(root, resolved),
    }
}

/// Make a path absolute against the working directory without touching the
/// filesystem, so the result stays in the same form as `root`.
fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path.to_path_buf(),
    }
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
    use super::{alias_static_path, normalize_path, SpaSource};
    use crate::embed::{EmbeddedAsset, EmbeddedAssets};
    use std::path::Path;

    const FILES: &[EmbeddedAsset] = &[EmbeddedAsset {
        path: "index.html",
        bytes: b"index",
        content_type: "text/html",
        etag: "\"sha256-1\"",
    }];

    fn embedded() -> SpaSource {
        SpaSource::Embedded {
            assets: EmbeddedAssets::new("wwwroot", FILES),
            overlay_root: "wwwroot".to_string(),
        }
    }

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

    /// Regression: a missing file must resolve to itself, not to `index.html`.
    ///
    /// Rewriting misses to the index hid every compiled-in file that had no disk
    /// counterpart, because the caller then served the index instead of trying
    /// the embedded table. It only showed up on Windows, where the old
    /// containment check compared a `\\?\`-prefixed canonical root against a
    /// plainly joined candidate and so reported a miss as a traversal attempt.
    #[test]
    fn missing_file_resolves_to_itself_not_the_index() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), b"shell").unwrap();
        let source = embedded().with_overlay_root(dir.path().to_string_lossy().into_owned());
        let mw = super::SpaMiddleware::from_source(source);

        let resolved = mw.resolve_file("/app.js").expect("inside the root");
        assert_eq!(resolved, dir.path().join("app.js"));
        assert!(!resolved.is_file(), "this is the miss case");

        // The index is still reachable for SPA routing, via the fallback the
        // caller applies once both the disk and embedded lookups miss.
        let index = mw.resolve_file("/").expect("inside the root");
        assert!(index.is_file());

        // Escaping the root stays rejected.
        assert_eq!(mw.resolve_file("/../secrets.txt"), None);
        assert_eq!(mw.resolve_file("/assets/../../app.db"), None);
    }

    #[test]
    fn skips_api_paths_for_spa_fallback() {
        // API paths are passed through to the router; no static/index.html response.
        // Full integration is covered by host tests; this documents the guard.
        assert!("/api/docs".starts_with("/api/"));
        assert!(!"/works/foo".starts_with("/api/"));
    }

    #[test]
    fn embedded_key_maps_root_to_index_and_applies_aliases() {
        let mw = super::SpaMiddleware::new(embedded());
        assert_eq!(mw.embedded_key("/").as_deref(), Some("index.html"));
        assert_eq!(
            mw.embedded_key("/assets/vendor/vditor-dist/dist/js/foo.js")
                .as_deref(),
            Some("assets/vendor/vditor-dist/js/foo.js")
        );
        assert_eq!(mw.embedded_key("some/page").as_deref(), Some("some/page"));
    }

    #[test]
    fn embedded_key_rejects_traversal() {
        let mw = super::SpaMiddleware::new(embedded());
        assert_eq!(mw.embedded_key("/../secrets.txt"), None);
        assert_eq!(mw.embedded_key("/assets/../../app.db"), None);
    }

    #[test]
    fn embedded_is_served_only_for_the_embedded_source() {
        let disk = super::SpaMiddleware::new(SpaSource::Disk("wwwroot".to_string()));
        assert!(disk.embedded().is_none());
        assert!(disk.embedded_asset("index.html").is_none());

        let with_embedded = super::SpaMiddleware::new(embedded());
        assert!(with_embedded.embedded().is_some());
        assert!(with_embedded.embedded_asset("index.html").is_some());
        assert!(with_embedded.embedded_asset("missing.html").is_none());
    }

    #[test]
    fn disk_root_and_build_root_are_independent() {
        // `use_spa("wwwroot")` then `embed_assets("frontend/dist")` in build.rs:
        // overrides come from wwwroot, the baseline is compiled from dist.
        let source = SpaSource::from(EmbeddedAssets::new("frontend/dist", FILES))
            .with_overlay_root("wwwroot");
        assert_eq!(source.disk_root(), "wwwroot");
        assert_eq!(source.embedded().unwrap().root(), "frontend/dist");
        assert_eq!(source.embedded().unwrap().len(), 1);
    }

    #[test]
    fn overlay_root_defaults_to_the_build_root() {
        let source = SpaSource::from(EmbeddedAssets::new("wwwroot", FILES));
        assert_eq!(source.disk_root(), "wwwroot");
    }

    #[test]
    fn with_overlay_root_replaces_a_disk_source() {
        let source = SpaSource::from("static").with_overlay_root("wwwroot");
        assert_eq!(source.disk_root(), "wwwroot");
        assert!(source.embedded().is_none());
    }

    #[test]
    fn embed_env_value_is_parsed() {
        use super::embed_enabled;
        // Unset, or anything not recognised as "off", keeps embedding on.
        assert!(embed_enabled(None));
        assert!(embed_enabled(Some("on")));
        assert!(embed_enabled(Some("embed")));
        // "off" is matched case-insensitively and after trimming whitespace.
        assert!(!embed_enabled(Some("off")));
        assert!(!embed_enabled(Some(" OFF  ")));
        assert!(!embed_enabled(Some("false")));
        assert!(!embed_enabled(Some("0")));
        assert!(!embed_enabled(Some("no")));
    }
}
