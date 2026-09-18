//! Static files compiled into the executable.
//!
//! This is the rust-webx counterpart of ASP.NET Core's
//! `ManifestEmbeddedFileProvider`: a `wwwroot` tree becomes a build-time table
//! of `&'static` byte slices, and the middleware serves it whenever the same
//! file is not present on disk. Deployment then consists of a single executable,
//! while an operator can still drop one or two files into `wwwroot/` next to it
//! to override what is baked in.
//!
//! Build a table with `rust_webx_build::embed_assets` in `build.rs`:
//!
//! ```ignore
//! // build.rs
//! fn main() -> Result<(), rust_webx_build::Error> {
//!     rust_webx_build::embed_assets("wwwroot")
//! }
//!
//! // src/main.rs
//! rust_webx::spa::embed_assets!();
//!
//! Host::builder()
//!     .use_spa("wwwroot")
//!     .embed()
//!     .build()
//! ```
//!
//! The build script names the directory the assets come from; `use_spa` names
//! the disk directory operators override through. The two can differ — build
//! from `frontend/dist` while overrides arrive in `wwwroot`.
//!
//! # Requirements on a hand-written table
//!
//! [`EmbeddedAssets::get`] binary-searches, so `files` **must be sorted by
//! `path`** in ascending byte order and must not contain duplicates.
//! [`EmbeddedAssets::debug_assert_sorted`] checks this in debug builds when the
//! middleware is constructed.

use std::path::Path;

/// One file compiled into the executable.
///
/// Produced by generated code; the fields are public so a build script can
/// emit the same table by hand.
#[derive(Debug, Clone, Copy)]
pub struct EmbeddedAsset {
    /// Normalized path relative to the embedded root, `/`-separated and without
    /// a leading slash (e.g. `assets/app.abc123.js`). Matched case-sensitively.
    pub path: &'static str,
    /// The file contents, placed in the binary's read-only data section.
    pub bytes: &'static [u8],
    /// Media type resolved at build time, so no runtime extension lookup is needed.
    pub content_type: &'static str,
    /// A strong, quoted `ETag` derived from the content.
    ///
    /// Embedded files have no mtime, so this is the only validator they can
    /// offer. Because it hashes the bytes it is also stable across machines and
    /// rebuilds, unlike the host's `size-mtime` tag for files on disk.
    pub etag: &'static str,
}

/// A build-time directory tree of static files.
#[derive(Debug, Clone, Copy)]
pub struct EmbeddedAssets {
    root: &'static str,
    files: &'static [EmbeddedAsset],
}

impl EmbeddedAssets {
    /// Wrap a table produced by generated code.
    ///
    /// `root` is the directory the bytes were compiled from, as written by the
    /// caller (e.g. `"wwwroot"`), and is used as the disk overlay directory when
    /// no `use_spa(...)` call named one.
    pub const fn new(root: &'static str, files: &'static [EmbeddedAsset]) -> Self {
        Self { root, files }
    }

    /// The directory the bytes were compiled from, as written by the caller.
    pub const fn root(&self) -> &'static str {
        self.root
    }

    /// The table, sorted by [`EmbeddedAsset::path`].
    pub const fn files(&self) -> &'static [EmbeddedAsset] {
        self.files
    }

    /// Number of embedded files.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Combined size of the embedded files, in bytes.
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes.len() as u64).sum()
    }

    /// Look up `path`, which must already be normalized: relative, `/`-separated,
    /// no leading slash and no `..` segments.
    pub fn get(&self, path: &str) -> Option<&'static EmbeddedAsset> {
        self.files
            .binary_search_by(|file| file.path.cmp(path))
            .ok()
            .map(|index| &self.files[index])
    }

    /// Panic in debug builds when the table is not sorted by `path`, which would
    /// make [`EmbeddedAssets::get`] miss entries instead of finding them.
    pub fn debug_assert_sorted(&self) {
        debug_assert!(
            self.files
                .windows(2)
                .all(|pair| pair[0].path < pair[1].path),
            "EmbeddedAssets must be sorted by path, with no duplicates"
        );
    }

    /// Embedded paths for which a same-named file exists under `disk_root`.
    ///
    /// Those are the files an operator is currently overriding. Overrides are
    /// the point of the feature, but a stale one silently defeats an updated
    /// build, so the host logs them at startup.
    pub fn shadowed_by(&self, disk_root: &Path) -> Vec<&'static str> {
        let mut shadowed = Vec::new();
        collect_shadowed(self, disk_root, "", &mut shadowed);
        shadowed
    }
}

fn collect_shadowed(
    assets: &EmbeddedAssets,
    dir: &Path,
    prefix: &str,
    shadowed: &mut Vec<&'static str>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let key = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        let path = entry.path();
        if path.is_dir() {
            collect_shadowed(assets, &path, &key, shadowed);
        } else if let Some(asset) = assets.get(&key) {
            shadowed.push(asset.path);
        }
    }
}

// Lets `rust_webx_build::embed_assets` register the table it generates, so
// `Host::builder().embed()` can pick it up without the application naming a
// value. Absence is fine: apps without embedded assets simply have no entry.
inventory::collect!(EmbeddedAssets);

#[cfg(test)]
mod tests {
    use super::{EmbeddedAsset, EmbeddedAssets};

    const FILES: &[EmbeddedAsset] = &[
        EmbeddedAsset {
            path: "assets/app.css",
            bytes: b"a{}",
            content_type: "text/css",
            etag: "\"sha256-1\"",
        },
        EmbeddedAsset {
            path: "index.html",
            bytes: b"index",
            content_type: "text/html",
            etag: "\"sha256-2\"",
        },
        EmbeddedAsset {
            path: "only-embedded.txt",
            bytes: b"embedded",
            content_type: "text/plain",
            etag: "\"sha256-3\"",
        },
    ];

    #[test]
    fn get_finds_every_entry() {
        let assets = EmbeddedAssets::new("wwwroot", FILES);
        assets.debug_assert_sorted();
        for file in FILES {
            assert_eq!(assets.get(file.path).unwrap().bytes, file.bytes);
        }
        assert_eq!(assets.len(), 3);
        assert_eq!(assets.total_bytes(), 16);
    }

    #[test]
    fn get_misses_unknown_and_prefix_paths() {
        let assets = EmbeddedAssets::new("wwwroot", FILES);
        assert!(assets.get("assets").is_none());
        assert!(assets.get("assets/nope.css").is_none());
        assert!(assets.get("index.htm").is_none());
        assert!(assets.get("").is_none());
    }

    #[test]
    fn root_is_reported_as_written() {
        assert_eq!(EmbeddedAssets::new("wwwroot", FILES).root(), "wwwroot");
        assert_eq!(
            EmbeddedAssets::new("../wwwroot", FILES).root(),
            "../wwwroot"
        );
    }

    #[test]
    fn shadowed_by_reports_same_named_disk_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/app.css"), b"disk").unwrap();
        std::fs::write(dir.path().join("only-embedded.txt"), b"disk").unwrap();
        std::fs::write(dir.path().join("only-disk.txt"), b"disk").unwrap();

        let mut shadowed = EmbeddedAssets::new("wwwroot", FILES).shadowed_by(dir.path());
        shadowed.sort_unstable();
        assert_eq!(shadowed, vec!["assets/app.css", "only-embedded.txt"]);
    }

    #[test]
    fn shadowed_by_missing_directory_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let assets = EmbeddedAssets::new("wwwroot", FILES);
        assert!(assets
            .shadowed_by(&dir.path().join("does-not-exist"))
            .is_empty());
    }
}
