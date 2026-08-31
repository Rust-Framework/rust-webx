//! Application base directory resolution.
//!
//! 解析应用基准目录（`app_base`）：appsettings.json 所在目录。
//! 框架的配置加载、SPA 静态资源定位均基于此目录，确保部署后从部署目录
//! 而非源码目录读取资源。
//!
//! 优先级：
//! 0. **环境变量 `RUST_WEBX_APP_BASE`**：显式指定基准目录（测试 / 容器挂载场景）。
//! 1. **exe 同级目录**：若 exe 所在目录存在 `appsettings.json`，视为部署目录
//!    （`cargo build --release` 后由发布脚本产出的布局，从任意 cwd 启动均生效）。
//! 2. **cwd 同级目录**：若当前工作目录存在 `appsettings.json`，视为基准目录
//!    （从部署目录启动 exe 的常见场景）。
//! 3. **cwd 向上遍历**：在 cwd 的各祖先的子目录中查找含 `appsettings.json`
//!    的目录（`cargo run` 从 workspace 根启动时定位 member crate 目录）。
//! 4. **回退 cwd**：以上均未命中时返回当前工作目录，由调用方按相对路径处理。

use std::path::{Path, PathBuf};

/// 判断给定目录是否为「应用基准目录」（存在 `appsettings.json`）。
///
/// `appsettings.json` 是框架配套配置文件的标志，存在即视为基准目录。
/// 不依赖 `wwwroot/`，因为并非所有应用都启用 SPA。
pub fn looks_like_app_base(dir: &Path) -> bool {
    dir.join("appsettings.json").exists()
}

/// 解析应用基准目录。优先级见模块文档。
pub fn app_base() -> PathBuf {
    // 0. 显式环境变量（测试 / 自定义部署路径）
    if let Ok(base) = std::env::var("RUST_WEBX_APP_BASE") {
        let path = PathBuf::from(&base);
        if path.is_dir() {
            return path;
        }
    }

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

    // 4. 回退 cwd
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Returns true when `dir` looks like the Rust-Framework monorepo root
/// (contains `rust-webx` and at least one other ecosystem crate).
pub fn looks_like_framework_root(dir: &Path) -> bool {
    dir.join("rust-webx").is_dir()
        && (dir.join("rust-ef").is_dir()
            || dir.join("rust-dix").is_dir()
            || dir.join("rust-agent-framework").is_dir()
            || dir.join("rust-gpui-rml").is_dir())
}

/// Resolve the Rust-Framework monorepo root for live sibling doc paths.
///
/// Priority:
/// 1. `RUST_FRAMEWORK_ROOT` environment variable
/// 2. Walk up from [`app_base`] looking for [`looks_like_framework_root`]
pub fn framework_root() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("RUST_FRAMEWORK_ROOT") {
        let path = PathBuf::from(&root);
        if path.is_dir() {
            return Some(path);
        }
    }

    let mut dir = app_base();
    for _ in 0..8 {
        if looks_like_framework_root(&dir) {
            return Some(dir);
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}
