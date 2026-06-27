//! Resolve filesystem paths for the docbit host crate.
//!
//! 路径解析策略（部署友好）：
//! 1. **exe 同级目录**：若 exe 所在目录同时存在 `appsettings.json` 与 `wwwroot/`，
//!    视为部署目录（`cargo build --release` 后由 publish.ps1 产出的布局）。
//! 2. **cwd 同级目录**：若当前工作目录存在 `appsettings.json` 与 `wwwroot/`，
//!    视为部署目录（从部署目录启动 exe 的常见场景）。
//! 3. **cwd 向上遍历**：在 cwd 的各祖先的子目录中查找含 `appsettings.json`+
//!    `wwwroot/` 的目录（`cargo run` 从 workspace 根启动时定位 `docbit/`）。
//! 4. **CARGO_MANIFEST_DIR 回退**：编译期常量，仅用于开发期兜底。
//!
//! `AppPaths` 与 `config` 模块均通过 [`app_base`] 获取基准目录，避免部署后
//! 仍读源码目录的问题。

use std::path::{Path, PathBuf};

/// Absolute path to the docbit-host crate root (`CARGO_MANIFEST_DIR`).
/// 仅用于开发期兜底，不应在部署场景依赖。
pub fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// 判断给定目录是否为「应用基准目录」（同时含 appsettings.json 与 wwwroot/）。
fn looks_like_app_base(dir: &Path) -> bool {
    dir.join("appsettings.json").exists() && dir.join("wwwroot").is_dir()
}

/// 解析应用基准目录。优先级见模块文档。
pub fn app_base() -> PathBuf {
    // 1. exe 同级目录（部署场景）
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            if looks_like_app_base(exe_dir) {
                return exe_dir.to_path_buf();
            }
        }
    }

    // 2. cwd 同级目录
    if let Ok(cwd) = std::env::current_dir() {
        if looks_like_app_base(&cwd) {
            return cwd;
        }

        // 3. cwd 向上遍历，在各祖先的子目录中查找
        let mut dir: Option<&Path> = Some(cwd.as_path());
        while let Some(d) = dir {
            if let Ok(entries) = std::fs::read_dir(d) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() && looks_like_app_base(&p) {
                        return p;
                    }
                }
            }
            dir = d.parent();
        }
    }

    // 4. 编译期 manifest 回退（开发期 cargo run）
    let manifest = manifest_dir();
    manifest
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or(manifest)
}

/// Resolved application paths — shared by hosted services and DI consumers.
#[derive(Clone)]
pub struct AppPaths {
    pub docs_root: PathBuf,
    pub db_path: PathBuf,
    pub wwwroot: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> Self {
        let base = app_base();
        Self {
            wwwroot: base.join("wwwroot"),
            db_path: base.join("docbit.db"),
            docs_root: resolve_data_path("docs", &base),
        }
    }
}

/// Resolve a relative data path (e.g. `docs`).
///
/// Search order:
/// 1. App base 目录 (`<base>/docs`)
/// 2. App base 的上一级 (`<workspace>/docs`)
/// 3. Walk cwd and parent directories
fn resolve_data_path(relative: &str, base: &Path) -> PathBuf {
    let rel = Path::new(relative);

    let direct = base.join(rel);
    if direct.exists() {
        return direct;
    }

    if let Some(workspace) = base.parent() {
        let workspace_path = workspace.join(rel);
        if workspace_path.exists() {
            return workspace_path;
        }
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
            dir = d.parent();
        }
    }

    direct
}
