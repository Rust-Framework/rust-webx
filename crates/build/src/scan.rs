//! One pass over the asset directory: what to embed, and what to watch.
//!
//! Traversal happens once and yields both the table entries and the paths cargo
//! must watch, so a single function owns the directory contract.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::error::Error;

/// A file compiled into the executable.
pub(crate) struct Asset {
    /// Path relative to the asset root, `/`-separated: the table key.
    pub(crate) key: String,
    /// Absolute path on disk, for `include_bytes!`.
    pub(crate) path: PathBuf,
    /// Media type inferred from the extension.
    pub(crate) content_type: String,
    /// Content-derived validator, quoted and ready to send.
    pub(crate) etag: String,
}

/// Everything one traversal of the asset directory yields.
pub(crate) struct Scan {
    /// Files to embed, ordered by key so generated code is reproducible.
    pub(crate) assets: Vec<Asset>,
    /// Every subdirectory visited, for `cargo:rerun-if-changed`.
    pub(crate) dirs: Vec<PathBuf>,
    /// Every file seen, for `cargo:rerun-if-changed`.
    pub(crate) files: Vec<PathBuf>,
}

impl Scan {
    /// Read `resolved`, which is `declared` after path resolution.
    ///
    /// `declared` is carried along only so a failure can name the path the
    /// build script actually wrote. Symbolic links are not followed in either
    /// direction: a link out of the tree would pull in files the manifest does
    /// not describe, and a linked directory could not be traversed safely.
    pub(crate) fn of(declared: &Path, resolved: &Path) -> Result<Self, Error> {
        if !resolved.is_dir() {
            return Err(Error::NotADirectory {
                declared: declared.to_path_buf(),
                resolved: resolved.to_path_buf(),
            });
        }

        let mut scan = Scan {
            assets: Vec::new(),
            dirs: Vec::new(),
            files: Vec::new(),
        };
        scan.visit(resolved, resolved)?;
        scan.assets.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(scan)
    }

    fn visit(&mut self, dir: &Path, base: &Path) -> Result<(), Error> {
        for entry in read_dir(dir)? {
            let entry = entry.map_err(|source| unreadable(dir, source))?;
            let file_type = entry
                .file_type()
                .map_err(|source| unreadable(dir, source))?;
            let path = entry.path();
            if file_type.is_dir() {
                self.dirs.push(path.clone());
                self.visit(&path, base)?;
            } else if file_type.is_file() {
                self.assets.push(Asset {
                    key: relative_key(&path, base),
                    content_type: content_type(&path),
                    etag: etag_for(&path)?,
                    path: path.clone(),
                });
                self.files.push(path);
            }
        }
        Ok(())
    }

    /// Ask cargo to re-run the build script when anything below `base` changes.
    ///
    /// The directory itself is listed first, then every subdirectory and file.
    /// Listing only the directory does not reliably catch files added inside a
    /// subdirectory, so the full set is emitted: added, edited and deleted
    /// files all retrigger the build, and the table cannot go stale.
    pub(crate) fn announce_rerun(&self, base: &Path) {
        println!("cargo:rerun-if-changed={}", base.display());
        for path in self.dirs.iter().chain(self.files.iter()) {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

/// Resolve `declared` against the calling crate's manifest directory.
///
/// `CARGO_MANIFEST_DIR` makes the path independent of the working directory,
/// which cargo does not standardize for build scripts. Outside a build script
/// the variable is absent, so the path is left as written for `Scan::of` to
/// report on.
pub(crate) fn resolve(declared: &Path) -> PathBuf {
    resolve_from(
        declared,
        std::env::var("CARGO_MANIFEST_DIR").ok().as_deref(),
    )
}

/// The join rule, with the environment passed in so it can be tested directly
/// rather than through a process-global variable.
fn resolve_from(declared: &Path, manifest_dir: Option<&str>) -> PathBuf {
    if declared.is_absolute() {
        return declared.to_path_buf();
    }
    match manifest_dir {
        Some(manifest_dir) => PathBuf::from(manifest_dir).join(declared),
        None => declared.to_path_buf(),
    }
}

fn read_dir(dir: &Path) -> Result<std::fs::ReadDir, Error> {
    std::fs::read_dir(dir).map_err(|source| unreadable(dir, source))
}

fn unreadable(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        operation: "read",
        path: path.to_path_buf(),
        source,
    }
}

/// Media type for a file, inferred from its extension.
///
/// The essence is kept bare (`text/css`, not `text/css; charset=utf-8`) because
/// the response layer owns the charset.
fn content_type(path: &Path) -> String {
    mime_guess::from_path(path)
        .first_or_octet_stream()
        .essence_str()
        .to_string()
}

/// A strong, quoted `ETag` derived from the file's contents.
///
/// Content-derived means it is stable across machines and rebuilds, so clients
/// and CDNs revalidate identically against every instance of the app — which
/// the disk file's size-mtime validator cannot offer.
fn etag_for(path: &Path) -> Result<String, Error> {
    use sha2::{Digest, Sha256};

    let bytes = std::fs::read(path).map_err(|source| unreadable(path, source))?;
    let digest = Sha256::digest(&bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(format!("\"sha256-{hex}\""))
}

/// `/`-separated key for a file, relative to the asset root.
///
/// Separators are normalized so a table built on Windows matches request paths,
/// which are always `/`-separated.
fn relative_key(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::{relative_key, resolve, resolve_from, Scan};
    use std::path::Path;

    fn tree(files: &[(&str, &[u8])]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (relative, contents) in files {
            let path = dir.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        dir
    }

    #[test]
    fn keys_are_relative_and_forward_slashed() {
        let dir = tree(&[("assets/js/app.js", b"a"), ("index.html", b"b")]);
        let scan = Scan::of(dir.path(), dir.path()).unwrap();
        let keys: Vec<&str> = scan.assets.iter().map(|a| a.key.as_str()).collect();
        assert_eq!(keys, ["assets/js/app.js", "index.html"]);
    }

    #[test]
    fn assets_carry_content_type_and_content_addressed_etag() {
        let dir = tree(&[
            ("app.css", b"body{}"),
            ("app.js", b"1"),
            ("logo.png", b"\x89PNG"),
        ]);
        let scan = Scan::of(dir.path(), dir.path()).unwrap();

        for asset in &scan.assets {
            assert!(
                asset.etag.starts_with("\"sha256-") && asset.etag.ends_with('"'),
                "{} should carry a quoted content hash, got {}",
                asset.key,
                asset.etag
            );
        }
        let css = scan.assets.iter().find(|a| a.key == "app.css").unwrap();
        assert_eq!(css.content_type, "text/css");
        // Same bytes ⇒ same validator, however the file was named or rebuilt.
        let again = Scan::of(dir.path(), dir.path()).unwrap();
        assert_eq!(
            css.etag,
            again
                .assets
                .iter()
                .find(|a| a.key == "app.css")
                .unwrap()
                .etag
        );
        // Different bytes ⇒ different validator.
        std::fs::write(dir.path().join("app.css"), b"body{color:red}").unwrap();
        let changed = Scan::of(dir.path(), dir.path()).unwrap();
        assert_ne!(
            css.etag,
            changed
                .assets
                .iter()
                .find(|a| a.key == "app.css")
                .unwrap()
                .etag
        );
    }

    #[test]
    fn every_subdirectory_and_file_is_listed_for_watching() {
        let dir = tree(&[("index.html", b"a"), ("assets/js/app.js", b"b")]);
        let scan = Scan::of(dir.path(), dir.path()).unwrap();

        let watched: Vec<String> = scan
            .dirs
            .iter()
            .chain(scan.files.iter())
            .map(|p| {
                p.strip_prefix(dir.path())
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert!(watched.contains(&"assets".to_string()), "got {watched:?}");
        assert!(
            watched.contains(&"assets/js".to_string()),
            "got {watched:?}"
        );
        assert!(
            watched.contains(&"assets/js/app.js".to_string()),
            "got {watched:?}"
        );
        assert!(
            watched.contains(&"index.html".to_string()),
            "got {watched:?}"
        );
    }

    #[test]
    fn a_missing_directory_is_an_error_that_names_both_paths() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no-such-assets");
        let Err(err) = Scan::of(Path::new("../no-such-assets"), &missing) else {
            panic!("a missing directory must be reported, not silently embedded as empty");
        };
        let message = err.to_string();
        assert!(message.contains("../no-such-assets"), "got {message}");
        assert!(message.contains("no-such-assets"), "got {message}");
        assert!(message.contains("embed_assets()"), "got {message}");
    }

    #[test]
    fn a_file_is_not_a_directory() {
        let dir = tree(&[("app.css", b"a")]);
        let file = dir.path().join("app.css");
        assert!(Scan::of(&file, &file).is_err());
    }

    #[test]
    fn resolve_joins_relative_paths_onto_the_manifest_dir() {
        assert_eq!(
            resolve_from(Path::new("wwwroot"), Some("/crate")),
            Path::new("/crate").join("wwwroot")
        );
        assert_eq!(
            resolve_from(Path::new("../wwwroot"), Some("/crate/host")),
            Path::new("/crate/host").join("../wwwroot")
        );
        // Outside a build script there is no manifest directory to join onto.
        assert_eq!(
            resolve_from(Path::new("wwwroot"), None),
            Path::new("wwwroot")
        );
    }

    #[test]
    fn an_absolute_path_is_used_as_written() {
        let absolute = tempfile::tempdir().unwrap();
        assert_eq!(resolve(absolute.path()), absolute.path());
        assert_eq!(
            resolve_from(absolute.path(), Some("/elsewhere")),
            absolute.path()
        );
    }

    #[test]
    fn relative_key_handles_nested_paths() {
        assert_eq!(
            relative_key(
                Path::new("root").join("a").join("b.css").as_path(),
                Path::new("root")
            ),
            "a/b.css"
        );
    }
}
