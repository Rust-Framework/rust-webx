//! Resolve filesystem paths for the docbit crate (data dirs, config, wwwroot).
//!
//! Kept in `common` so services and bootstrap share one lookup strategy instead of
//! scattering `CARGO_MANIFEST_DIR` / cwd logic across `main.rs`.

use std::path::{Path, PathBuf};

/// Absolute path to the docbit crate root (`CARGO_MANIFEST_DIR`).
pub fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Resolve a relative path (e.g. `docs`, `appsettings.json`).
///
/// Search order:
/// 1. Workspace root (`../docs` relative to the docbit crate)
/// 2. Docbit crate directory (`docbit/docs`)
/// 3. Walk cwd and parent directories
pub fn resolve_data_path(relative: impl AsRef<Path>) -> PathBuf {
    let relative = relative.as_ref();

    if let Some(workspace) = manifest_dir().parent() {
        let workspace_path = workspace.join(relative);
        if workspace_path.exists() {
            return workspace_path;
        }
    }

    let direct = manifest_dir().join(relative);
    if direct.exists() {
        return direct;
    }

    if let Ok(cwd) = std::env::current_dir() {
        let mut dir = Some(cwd.as_path());
        while let Some(d) = dir {
            if let Ok(entries) = std::fs::read_dir(d) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        let candidate = entry.path().join(relative);
                        if candidate.exists() {
                            return candidate;
                        }
                    }
                }
            }
            let in_crate = d.join("docbit").join(relative);
            if in_crate.exists() {
                return in_crate;
            }
            dir = d.parent();
        }
    }

    direct
}
