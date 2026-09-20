//! Build-script support for compiling static files into the executable.
//!
//! The package name on crates.io is `rust-webx-build`; the library is
//! [`webx`] so `build.rs` reads like the rest of the framework.
//!
//! # Payload sizes
//!
//! Compressible files are brotli-encoded (quality 11) before they enter the
//! table, and the middleware serves them with `Content-Encoding: br` when the
//! client accepts it. Media that is already compressed (PNG, WOFF2, ZIP, …) and
//! files below 256 bytes are stored verbatim, because a second compression pass
//! cannot shrink them. On a typical `wwwroot` this removes roughly 80% of the
//! embedded footprint.
//!
//! # Three deployment shapes
//!
//! | Setup | Result |
//! |-------|--------|
//! | No `build.rs` web root, `#[webx::main]` | Disk SPA only (`.use_spa` or auto-detect `wwwroot/`) |
//! | `build.rs` `web_root` + `#[webx::main(embed)]` | Files baked into the exe; disk overlay optional |
//! | `build.rs` without `(embed)` on main | Table is generated but **not** linked — nothing baked in |
//!
//! ## Terms
//!
//! - **`web_root` (build.rs)** — compile-time source tree copied into the binary  
//! - **`.use_spa(dir)` (runtime)** — disk directory checked first (operator overrides).
//!   Often the same name (`"wwwroot"`) but a different path than `web_root`
//!   (e.g. compile from `../wwwroot`, deploy overlay `wwwroot` next to the exe).
//!
//! ```ignore
//! // build.rs
//! fn main() -> Result<(), webx::Error> {
//!     webx::builder()
//!         .web_root("wwwroot")
//!         .build()
//! }
//!
//! // main.rs
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
//! | Where | Call | Role |
//! |-------|------|------|
//! | `build.rs` | [`builder`] → [`Builder::web_root`] → [`Builder::build`] | write the table |
//! | `main` | `#[webx::main(embed)]` | link the table into this binary |
//! | runtime | `Host::build` | serve table under the disk overlay |
//!
//! # What belongs in a build script
//!
//! Compiling assets in is a build-time decision because the bytes are fixed when
//! the binary is produced. Runtime concerns (settings, origins, secrets, TLS)
//! stay out of this crate on purpose.
//!
//! # Failures are values
//!
//! A missing asset directory is an [`Error`], so `build.rs` can return it from
//! `main` and let cargo attribute the failure to the build.
//!
//! ```ignore
//! [build-dependencies]
//! rust-webx-build = "0.5"   # imports as `webx`
//! ```

mod error;
mod render;
mod scan;

use std::path::{Path, PathBuf};

pub use error::Error;

/// Name of the file written to `OUT_DIR`, and read back by
/// `#[webx::main(embed)]` / `webx::spa::embed_assets!()`.
///
/// Spelled out in the runtime macro because a build dependency cannot be
/// reached from `rust-webx-spa`; a mismatch fails at compile time.
pub const GENERATED_FILE: &str = "webx_embedded_assets.rs";

/// Start a compile-time web-root builder (call from `build.rs` only).
pub fn builder() -> Builder {
    Builder::new()
}

/// Fluent compile-time configuration for embedding a static-file directory.
///
/// `web_root` names the **compile-time** tree. Pair with
/// `#[webx::main(embed)]` and usually `.use_spa("wwwroot")` at runtime.
#[derive(Debug, Default)]
pub struct Builder {
    web_root: Option<PathBuf>,
}

impl Builder {
    /// Empty builder; set [`web_root`](Self::web_root) before [`build`](Self::build).
    pub fn new() -> Self {
        Self { web_root: None }
    }

    /// Directory of files to bake into the binary (resolved against
    /// `CARGO_MANIFEST_DIR`).
    ///
    /// Compressible files are brotli-encoded into the generated table; see the
    /// crate docs for what that does to the binary.
    pub fn web_root(mut self, dir: impl AsRef<Path>) -> Self {
        self.web_root = Some(dir.as_ref().to_path_buf());
        self
    }

    /// Scan the web root, write the asset table to `OUT_DIR`, and register
    /// `cargo:rerun-if-changed` watchers.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotADirectory`] when the web root is missing,
    /// [`Error::Io`] on filesystem failures, [`Error::MissingOutDir`] outside a
    /// build script, and [`Error::MissingWebRoot`] when [`web_root`](Self::web_root)
    /// was never set.
    pub fn build(self) -> Result<(), Error> {
        let declared = self.web_root.ok_or(Error::MissingWebRoot)?;
        embed_assets(declared)
    }
}

/// Compile `dir` into a table of static assets for this crate's binary.
///
/// Prefer [`builder`] for new code. This remains as a one-shot equivalent of
/// `builder().web_root(dir).build()`.
pub fn embed_assets(dir: impl AsRef<Path>) -> Result<(), Error> {
    let declared = dir.as_ref();
    let resolved = scan::resolve(declared);

    let scan = scan::Scan::of(declared, &resolved)?;
    let out_dir = out_dir()?;
    let payload_dir = render::payloads(&out_dir, &scan.assets)?;
    let generated = render::table(declared, &scan.assets, &payload_dir);

    let target = out_dir.join(GENERATED_FILE);
    std::fs::write(&target, generated).map_err(|source| Error::Io {
        operation: "write",
        path: target,
        source,
    })?;

    scan.announce_rerun(&resolved);
    Ok(())
}

fn out_dir() -> Result<PathBuf, Error> {
    std::env::var("OUT_DIR")
        .map(PathBuf::from)
        .map_err(|_| Error::MissingOutDir)
}
