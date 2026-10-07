//! Static files compiled into the executable.
//!
//! This is the rust-webx counterpart of ASP.NET Core's
//! `ManifestEmbeddedFileProvider`: a `wwwroot` tree becomes a build-time table
//! of `&'static` byte slices, and the middleware serves it whenever the same
//! file is not present on disk. Deployment then consists of a single executable,
//! while an operator can still drop one or two files into `wwwroot/` next to it
//! to override what is baked in.
//!
//! Build a table with `webx::builder()` in `build.rs`, then opt in at the
//! entry point with `#[webx::main(embed)]`:
//!
//! ```ignore
//! // build.rs — compile-time source tree (baked into the binary)
//! fn main() -> Result<(), webx::Error> {
//!     webx::builder()
//!         .web_root("wwwroot")
//!         .build()
//! }
//!
//! // main.rs — (embed) links the table; use_spa is the runtime disk overlay
//! #[webx::main(embed)]
//! async fn main() {
//!     Host::builder()
//!         .use_spa("wwwroot")
//!         .build()
//!         .run()
//!         .await
//!         .unwrap();
//! }
//! ```
//!
//! Without `(embed)` (and without `build.rs` web root), SPA is disk-only.
//! `web_root` and `use_spa` can differ — build from `frontend/dist`, overrides
//! in deploy-time `wwwroot`.
//!
//! # Payload encoding
//!
//! Compressible files are stored brotli-encoded ([`ContentEncoding::Brotli`])
//! alongside their original length, which is what stops a large `wwwroot` from
//! becoming a large binary. The middleware hands the stored stream straight to
//! clients that accept `br`, and only decodes — once per file, then cached — for
//! clients that do not. Media that is already compressed stays verbatim
//! ([`ContentEncoding::Identity`]).
//!
//! The build script decides all of this per file. A hand-written table must
//! keep the two fields consistent: `raw_len` is the decoded length, and
//! `bytes.len() == raw_len` whenever `encoding` is `Identity`.
//!
//! # Reading files at runtime
//!
//! [`crate::assets`] reads the registered table by path, anywhere and without a
//! `Host`, returning the original contents — decoded once if the file is
//! stored compressed:
//!
//! ```ignore
//! let template = webx::assets::read_str("templates/mail.html");
//! ```
//!
//! [`EmbeddedAssets::registered`] with [`EmbeddedAssets::read`] /
//! [`EmbeddedAssets::read_str`] is the same lookup on an explicit table.
//!
//! # Requirements on a hand-written table
//!
//! [`EmbeddedAssets::get`] binary-searches, so `files` **must be sorted by
//! `path`** in ascending byte order and must not contain duplicates.
//! [`EmbeddedAssets::debug_assert_sorted`] checks this in debug builds when the
//! middleware is constructed.

use std::path::Path;

/// How an [`EmbeddedAsset`]'s payload is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentEncoding {
    /// The payload is the file itself.
    Identity,
    /// The payload is a brotli stream; `raw_len` is the decoded length.
    Brotli,
}

impl ContentEncoding {
    /// The `Content-Encoding` token the payload is stored in, if any.
    pub const fn token(self) -> Option<&'static str> {
        match self {
            Self::Identity => None,
            Self::Brotli => Some("br"),
        }
    }
}

/// One file compiled into the executable.
///
/// Produced by generated code; the fields are public so a build script can
/// emit the same table by hand.
#[derive(Debug, Clone, Copy)]
pub struct EmbeddedAsset {
    /// Normalized path relative to the embedded root, `/`-separated and without
    /// a leading slash (e.g. `assets/app.abc123.js`). Matched case-sensitively.
    pub path: &'static str,
    /// The stored payload: the file for [`ContentEncoding::Identity`], its
    /// brotli stream for [`ContentEncoding::Brotli`]. Placed in the binary's
    /// read-only data section.
    pub bytes: &'static [u8],
    /// Length of the file before compression. Equals `bytes.len()` when the
    /// payload is stored verbatim.
    pub raw_len: usize,
    /// How `bytes` is encoded.
    pub encoding: ContentEncoding,
    /// Media type resolved at build time, so no runtime extension lookup is needed.
    pub content_type: &'static str,
    /// A strong, quoted `ETag` derived from the content.
    ///
    /// Embedded files have no mtime, so this is the only validator they can
    /// offer. Because it hashes the bytes it is also stable across machines and
    /// rebuilds, unlike the host's `size-mtime` tag for files on disk.
    pub etag: &'static str,
}

impl EmbeddedAsset {
    /// Decode the stored payload back to the original file.
    ///
    /// `None` only when an encoded payload fails to decode, which means the
    /// table is corrupt; the caller still has [`bytes`](Self::bytes) to serve.
    pub fn decode(&self) -> Option<Vec<u8>> {
        match self.encoding {
            ContentEncoding::Identity => Some(self.bytes.to_vec()),
            ContentEncoding::Brotli => decode_brotli(self.bytes, self.raw_len),
        }
    }

    /// Whether the payload is stored compressed.
    pub fn is_encoded(&self) -> bool {
        matches!(self.encoding, ContentEncoding::Brotli)
    }

    /// The strong validator for one representation of this asset.
    ///
    /// A compressed payload is a different representation, so it needs its own
    /// `ETag`: reusing the identity tag would let a shared cache hand brotli
    /// bytes to a client that asked for the plain file.
    pub fn etag_for(&self, encoded: bool) -> String {
        if !encoded {
            return self.etag.to_string();
        }
        match self.etag.strip_suffix('"') {
            Some(head) => format!("{head}-br\""),
            None => format!("{}-br", self.etag),
        }
    }
}

/// Decode one brotli stream, reserving the original length up front.
fn decode_brotli(payload: &[u8], raw_len: usize) -> Option<Vec<u8>> {
    use std::io::Read as _;

    let mut out = Vec::with_capacity(raw_len);
    let mut decoder = brotli_decompressor::Decompressor::new(payload, 4096);
    decoder.read_to_end(&mut out).ok()?;
    Some(out)
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

    /// Combined size of the stored payloads, in bytes — what the binary holds.
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes.len() as u64).sum()
    }

    /// Combined size of the files as they exist on disk.
    ///
    /// Larger than [`total_bytes`](Self::total_bytes) whenever something was
    /// compressed; the difference is what brotli saved.
    pub fn total_raw_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.raw_len as u64).sum()
    }

    /// Look up `path`, which must already be normalized: relative, `/`-separated,
    /// no leading slash and no `..` segments.
    pub fn get(&self, path: &str) -> Option<&'static EmbeddedAsset> {
        self.files
            .binary_search_by(|file| file.path.cmp(path))
            .ok()
            .map(|index| &self.files[index])
    }

    /// The table linked by `#[webx::main(embed)]` (or `#[webx::embed_assets]`),
    /// if any — the same one the host serves, reachable without a `Host`.
    ///
    /// To read a file by path, [`crate::assets::read`] is the shorter form.
    ///
    /// # Panics
    ///
    /// When more than one table is registered — that is always a wiring mistake
    /// (e.g. both `#[webx::main(embed)]` and a second `#[webx::embed_assets]`
    /// in the same binary).
    pub fn registered() -> Option<&'static EmbeddedAssets> {
        let mut found = inventory::iter::<EmbeddedAssets>();
        match (found.next(), found.next()) {
            (Some(assets), None) => Some(assets),
            (None, _) => None,
            (Some(_), Some(_)) => panic!(
                "more than one compiled-in asset table is registered; \
                 use `#[webx::main(embed)]` (or `#[webx::embed_assets]`) \
                 exactly once per binary"
            ),
        }
    }

    /// The original contents of `path`, decoded when stored compressed.
    ///
    /// `path` is relative to the embedded root; a leading `/` is accepted so a
    /// request path works as-is. Verbatim entries are returned without a copy;
    /// compressed ones are decoded on first use and cached for the life of the
    /// process, shared with the middleware. `None` when the file is not
    /// embedded or its payload fails to decode.
    ///
    /// Only the compiled-in copy is read: a same-named file in the disk overlay
    /// and [`EMBED_ENV`](crate::EMBED_ENV) play no part.
    pub fn read(&self, path: &str) -> Option<&'static [u8]> {
        let asset = self.get(path.trim_start_matches('/'))?;
        match asset.encoding {
            ContentEncoding::Identity => Some(asset.bytes),
            ContentEncoding::Brotli => crate::spa::decoded_once(asset),
        }
    }

    /// [`read`](Self::read) as UTF-8 text; also `None` when it is not UTF-8.
    pub fn read_str(&self, path: &str) -> Option<&'static str> {
        std::str::from_utf8(self.read(path)?).ok()
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

// Lets `webx::builder()` register the table it generates, so
// `Host::build` can pick it up without the application naming a value.
// Absence is fine: apps without embedded assets simply have no entry.
inventory::collect!(EmbeddedAssets);

#[cfg(test)]
mod tests {
    use super::{ContentEncoding, EmbeddedAsset, EmbeddedAssets};

    const FILES: &[EmbeddedAsset] = &[
        EmbeddedAsset {
            path: "assets/app.css",
            bytes: b"a{}",
            raw_len: 3,
            encoding: ContentEncoding::Identity,
            content_type: "text/css",
            etag: "\"sha256-1\"",
        },
        EmbeddedAsset {
            path: "index.html",
            bytes: b"index",
            raw_len: 5,
            encoding: ContentEncoding::Identity,
            content_type: "text/html",
            etag: "\"sha256-2\"",
        },
        EmbeddedAsset {
            path: "only-embedded.txt",
            bytes: b"embedded",
            raw_len: 8,
            encoding: ContentEncoding::Identity,
            content_type: "text/plain",
            etag: "\"sha256-3\"",
        },
    ];

    /// Brotli-encode `source`, the way the build script would.
    fn brotli(source: &str) -> Vec<u8> {
        use std::io::Write as _;

        let mut out = Vec::new();
        let mut encoder = brotli::CompressorWriter::new(&mut out, 4096, 11, 22);
        encoder.write_all(source.as_bytes()).unwrap();
        encoder.flush().unwrap();
        drop(encoder);
        out
    }

    #[test]
    fn get_finds_every_entry() {
        let assets = EmbeddedAssets::new("wwwroot", FILES);
        assets.debug_assert_sorted();
        for file in FILES {
            assert_eq!(assets.get(file.path).unwrap().bytes, file.bytes);
        }
        assert_eq!(assets.len(), 3);
        assert_eq!(assets.total_bytes(), 16);
        assert_eq!(assets.total_raw_bytes(), 16);
    }

    #[test]
    fn an_encoded_entry_decodes_back_to_the_original_file() {
        let source = "body { color: red; }".repeat(32);
        let payload = brotli(&source);
        assert!(
            payload.len() < source.len(),
            "the fixture must actually compress"
        );

        let encoded = EmbeddedAsset {
            path: "app.css",
            bytes: Box::leak(payload.into_boxed_slice()),
            raw_len: source.len(),
            encoding: ContentEncoding::Brotli,
            content_type: "text/css",
            etag: "\"sha256-x\"",
        };
        assert!(encoded.is_encoded());
        assert_eq!(encoded.decode().unwrap(), source.as_bytes());

        // Identity entries decode to themselves.
        assert_eq!(FILES[0].decode().unwrap(), b"a{}");
        assert!(!FILES[0].is_encoded());
    }

    #[test]
    fn totals_separate_the_stored_payload_from_the_original_size() {
        let source = "x".repeat(4096);
        let payload = brotli(&source);
        let encoded: &'static EmbeddedAsset = Box::leak(Box::new(EmbeddedAsset {
            path: "big.txt",
            bytes: Box::leak(payload.into_boxed_slice()),
            raw_len: 4096,
            encoding: ContentEncoding::Brotli,
            content_type: "text/plain",
            etag: "\"sha256-y\"",
        }));
        let assets = EmbeddedAssets::new("wwwroot", std::slice::from_ref(encoded));
        assert!(assets.total_bytes() < assets.total_raw_bytes());
        assert_eq!(assets.total_raw_bytes(), 4096);
    }

    #[test]
    fn each_representation_gets_its_own_validator() {
        let plain = &FILES[0];
        assert_eq!(plain.etag_for(false), "\"sha256-1\"");
        assert_eq!(plain.etag_for(true), "\"sha256-1-br\"");
        assert_ne!(plain.etag_for(false), plain.etag_for(true));

        // A hand-written table without the surrounding quotes still yields a
        // distinct, well-formed tag rather than a truncated one.
        let bare = EmbeddedAsset {
            etag: "sha256-z",
            ..*plain
        };
        assert_eq!(bare.etag_for(true), "sha256-z-br");
    }

    #[test]
    fn encoding_tokens_are_the_http_ones() {
        assert_eq!(ContentEncoding::Identity.token(), None);
        assert_eq!(ContentEncoding::Brotli.token(), Some("br"));
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
    fn read_returns_verbatim_entries_without_a_copy() {
        let assets = EmbeddedAssets::new("wwwroot", FILES);
        let bytes = assets.read("index.html").unwrap();
        assert!(std::ptr::eq(bytes, FILES[1].bytes));
        // A request path works as-is.
        assert_eq!(assets.read("/assets/app.css").unwrap(), b"a{}");
        assert_eq!(assets.read_str("only-embedded.txt"), Some("embedded"));
        assert!(assets.read("missing.txt").is_none());
    }

    #[test]
    fn read_decodes_compressed_entries_once() {
        let source = "p { margin: 0; }".repeat(64);
        let encoded: &'static EmbeddedAsset = Box::leak(Box::new(EmbeddedAsset {
            path: "read-decoded.css",
            bytes: Box::leak(brotli(&source).into_boxed_slice()),
            raw_len: source.len(),
            encoding: ContentEncoding::Brotli,
            content_type: "text/css",
            etag: "\"sha256-r\"",
        }));
        let assets = EmbeddedAssets::new("wwwroot", std::slice::from_ref(encoded));

        let first = assets.read_str("read-decoded.css").unwrap();
        assert_eq!(first, source);
        let second = assets.read_str("read-decoded.css").unwrap();
        assert!(std::ptr::eq(first.as_ptr(), second.as_ptr()));
    }

    #[test]
    fn read_str_rejects_non_utf8() {
        let binary = EmbeddedAsset {
            path: "logo.bin",
            bytes: &[0xff, 0xfe],
            raw_len: 2,
            encoding: ContentEncoding::Identity,
            content_type: "application/octet-stream",
            etag: "\"sha256-b\"",
        };
        let files: &'static [EmbeddedAsset] = Box::leak(Box::new([binary]));
        let assets = EmbeddedAssets::new("wwwroot", files);
        assert_eq!(assets.read("logo.bin"), Some(&[0xff, 0xfe][..]));
        assert!(assets.read_str("logo.bin").is_none());
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
