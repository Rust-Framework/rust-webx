//! One pass over the asset directory: what to embed, and what to watch.
//!
//! Traversal happens once and yields both the table entries and the paths cargo
//! must watch, so a single function owns the directory contract.
//!
//! Compressible files are brotli-encoded here, so the bytes that reach the
//! binary are the compressed form and the raw file never enters the executable.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::error::Error;

/// Brotli quality. 11 is the maximum: it costs build time, not runtime size or
/// startup, so it is the right trade for a value fixed at compile time.
const BROTLI_QUALITY: u32 = 11;

/// Brotli window (`lgwin`). 24 keeps multi-megabyte bundles inside one window,
/// which is where most of the ratio on vendored JS comes from.
const BROTLI_WINDOW: u32 = 24;

/// Files smaller than this are stored verbatim: a brotli frame header costs
/// more than the body it would wrap.
const MIN_COMPRESSIBLE: u64 = 256;

/// Extensions that are already compressed on disk. Recompressing them cannot
/// shrink them, so they are stored verbatim instead of burning build time.
const ALREADY_COMPRESSED: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "ico", "webp", "avif", "woff", "woff2", "zip", "gz", "br", "xz",
    "7z", "zst", "mp3", "mp4", "webm", "pdf",
];

/// A file compiled into the executable.
pub(crate) struct Asset {
    /// Path relative to the asset root, `/`-separated: the table key.
    pub(crate) key: String,
    /// Absolute path on disk, for `include_bytes!` when stored verbatim.
    pub(crate) path: PathBuf,
    /// Media type inferred from the extension.
    pub(crate) content_type: String,
    /// Content-derived validator, quoted and ready to send.
    pub(crate) etag: String,
    /// Size of the file as it exists on disk — the decoded length.
    pub(crate) raw_len: u64,
    /// The brotli stream, when compressing was worth it. `None` means the
    /// payload is the original file, referenced by `include_bytes!`.
    pub(crate) compressed: Option<Vec<u8>>,
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
                self.assets.push(asset(&path, base)?);
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

/// Read one file and describe it for the generated table.
///
/// The file is read once: the same bytes produce the `ETag` and, when it pays
/// off, the compressed payload.
fn asset(path: &Path, base: &Path) -> Result<Asset, Error> {
    let bytes = std::fs::read(path).map_err(|source| unreadable(path, source))?;
    let raw_len = bytes.len() as u64;
    Ok(Asset {
        key: relative_key(path, base),
        content_type: content_type(path),
        etag: etag_of(&bytes),
        path: path.to_path_buf(),
        raw_len,
        compressed: compress(path, &bytes),
    })
}

/// A strong, quoted `ETag` derived from the file's contents.
///
/// Content-derived means it is stable across machines and rebuilds, so clients
/// and CDNs revalidate identically against every instance of the app — which
/// the disk file's size-mtime validator cannot offer.
fn etag_of(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    format!("\"sha256-{hex}\"")
}

/// Brotli-compress `bytes` when it is worth doing, otherwise `None`.
///
/// A payload that does not come back smaller is dropped: a table entry holding
/// a larger "compressed" copy than the file it replaced would be a regression,
/// and that is possible on tiny or already-entropic files.
fn compress(path: &Path, bytes: &[u8]) -> Option<Vec<u8>> {
    if (bytes.len() as u64) < MIN_COMPRESSIBLE || is_already_compressed(path) {
        return None;
    }
    let compressed = brotli(bytes)?;
    (compressed.len() < bytes.len()).then_some(compressed)
}

/// Whether `path`'s extension is one that compression cannot improve.
fn is_already_compressed(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ALREADY_COMPRESSED
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        })
}

/// One-shot brotli encode at [`BROTLI_QUALITY`].
///
/// `None` when the encoder fails, which is not fatal: the caller falls back to
/// storing the file verbatim.
fn brotli(bytes: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write as _;

    let mut out = Vec::with_capacity(bytes.len() / 3);
    let mut encoder = brotli::CompressorWriter::new(&mut out, 4096, BROTLI_QUALITY, BROTLI_WINDOW);
    encoder.write_all(bytes).ok()?;
    encoder.flush().ok()?;
    drop(encoder);
    Some(out)
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
    use super::{is_already_compressed, relative_key, resolve, resolve_from, Scan};
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
        assert!(message.contains("web_root"), "got {message}");
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

    #[test]
    fn compressible_files_are_stored_compressed_and_media_is_not() {
        let script = b"export const answer = 42;\n".repeat(64);
        let dir = tree(&[
            ("app.js", script.as_slice()),
            ("logo.png", b"\x89PNG\r\n\x1a\n binary payload"),
            (
                "site.webmanifest",
                b"{\"name\":\"docbit\"}\n".repeat(32).as_slice(),
            ),
        ]);
        let scan = Scan::of(dir.path(), dir.path()).unwrap();
        let find = |key: &str| scan.assets.iter().find(|a| a.key == key).unwrap();
        let payload_len = |key: &str| find(key).compressed.as_ref().unwrap().len();

        let js = find("app.js");
        assert!(js.compressed.is_some(), "repetitive JS must be compressed");
        assert_eq!(js.raw_len, script.len() as u64);
        assert!((payload_len("app.js") as u64) < js.raw_len);

        // Already compressed on disk: re-encoding cannot shrink it.
        let png = find("logo.png");
        assert!(png.compressed.is_none());
        assert_eq!(
            png.raw_len,
            b"\x89PNG\r\n\x1a\n binary payload".len() as u64
        );

        // A `.webmanifest` is a text asset even though the extension looks odd.
        assert!(find("site.webmanifest").compressed.is_some());
    }

    #[test]
    fn tiny_and_incompressible_files_are_stored_verbatim() {
        let dir = tree(&[("small.txt", b"hi")]);
        let scan = Scan::of(dir.path(), dir.path()).unwrap();
        let small = &scan.assets[0];
        assert!(
            small.compressed.is_none(),
            "a frame header would exceed the body"
        );
        assert_eq!(small.raw_len, 2);
    }

    #[test]
    fn encoding_is_recognised_case_insensitively() {
        assert!(is_already_compressed(Path::new("a/b.PNG")));
        assert!(is_already_compressed(Path::new("a/b.woff2")));
        assert!(!is_already_compressed(Path::new("a/b.js")));
        assert!(!is_already_compressed(Path::new("no-extension")));
    }
}
