//! Resolve filesystem paths for the docbit host crate.
//!
//! 与 `docbit/src/common/paths.rs` 等价：基于 `CARGO_MANIFEST_DIR` 解析
//! `wwwroot`、`docbit.db`、`docs/`、`blog-data/` 等路径。

use std::path::{Path, PathBuf};

/// Absolute path to the docbit-host crate root (`CARGO_MANIFEST_DIR`).
pub fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Resolved application paths — shared by hosted services and DI consumers.
#[derive(Clone)]
pub struct AppPaths {
    pub docs_root: PathBuf,
    pub blog_root: PathBuf,
    pub db_path: PathBuf,
    pub wwwroot: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> Self {
        let manifest = manifest_dir();
        // host crate 位于 `docbit/host/`，故上一级即为 docbit 目录。
        let docbit_root = manifest.parent().unwrap_or(&manifest);
        Self {
            wwwroot: docbit_root.join("wwwroot"),
            db_path: docbit_root.join("docbit.db"),
            docs_root: resolve_data_path("docs", docbit_root),
            blog_root: resolve_data_path("blog-data", docbit_root),
        }
    }
}

/// Resolve a relative path (e.g. `docs`, `appsettings.json`).
///
/// Search order:
/// 1. Workspace root (`../docs` relative to the docbit crate)
/// 2. Docbit crate directory (`docbit/docs`)
/// 3. Walk cwd and parent directories
fn resolve_data_path(relative: &str, docbit_root: &Path) -> PathBuf {
    let rel = Path::new(relative);

    if let Some(workspace) = docbit_root.parent() {
        let workspace_path = workspace.join(rel);
        if workspace_path.exists() {
            return workspace_path;
        }
    }

    let direct = docbit_root.join(rel);
    if direct.exists() {
        return direct;
    }

    if let Ok(cwd) = std::env::current_dir() {
        let mut dir: Option<&Path> = Some(cwd.as_path());
        while let Some(d) = dir {
            if let Ok(entries) = std::fs::read_dir(d) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        let candidate = entry.path().join(rel);
                        if candidate.exists() {
                            return candidate;
                        }
                    }
                }
            }
            let in_crate = d.join("docbit").join(rel);
            if in_crate.exists() {
                return in_crate;
            }
            dir = d.parent();
        }
    }

    direct
}
