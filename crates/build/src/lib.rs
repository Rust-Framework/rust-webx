//! Build-script support for compiling static files into the executable.
//!
//! One story, three call sites:
//!
//! | Where | Call | Role |
//! |-------|------|------|
//! | `build.rs` | [`embed_assets`] | read the directory, write the table |
//! | `src/main.rs` | `rust_webx::spa::embed_assets!()` | include the table |
//! | runtime | `.embed()` | serve the table under the disk overlay |
//!
//! ```ignore
//! // build.rs
//! fn main() -> Result<(), rust_webx_build::Error> {
//!     rust_webx_build::embed_assets("wwwroot")
//! }
//! ```
//!
//! ```ignore
//! // src/main.rs
//! rust_webx::spa::embed_assets!();
//!
//! #[tokio::main]
//! async fn main() -> std::io::Result<()> {
//!     Host::builder()
//!         .use_spa("wwwroot")   // optional: where operators may drop overrides
//!         .embed()              // serve the compiled-in files underneath
//!         .build()
//!         .run()
//!         .await
//! }
//! ```
//!
//! # What belongs in a build script
//!
//! Compiling assets in is a build-time decision because the bytes are fixed when
//! the binary is produced: the file set, their media types, their validators and
//! the SPA shell. Anything an operator may change after deploying — settings,
//! origins, secrets, TLS material — must stay a runtime concern, which is why
//! this crate deliberately has no configuration surface.
//!
//! # Failures are values
//!
//! Following the framework's runtime convention, a missing asset directory is
//! reported as an [`Error`] rather than a panic, so `build.rs` can return it from
//! `main` and let cargo attribute the failure to the build. `Error` renders
//! through [`Display`](std::fmt::Display) even in `Debug`, because that is the
//! form cargo prints.
//!
//! # Generated code
//!
//! The table is written to `OUT_DIR` as [`GENERATED_FILE`] and refers to
//! `::rust_webx`, so the crate that calls [`embed_assets`] must depend on the
//! umbrella crate — the documented way to build an app.
//!
//! ```ignore
//! [build-dependencies]
//! rust-webx-build = "0.4"
//! ```

mod error;
mod render;
mod scan;

use std::path::Path;

pub use error::Error;

/// Name of the file written to `OUT_DIR`, and read back by
/// `rust_webx::spa::embed_assets!()`.
///
/// The runtime macro spells the same name out because a build dependency cannot
/// be reached from `rust-webx-spa`; the two sides name this constant to keep the
/// include working, and a mismatch fails at compile time rather than silently.
pub const GENERATED_FILE: &str = "rust_webx_embedded_assets.rs";

/// Compile `dir` into a table of static assets for this crate's binary.
///
/// `dir` is resolved against the calling crate's `CARGO_MANIFEST_DIR`, so it does
/// not depend on the working directory. Every file becomes an `include_bytes!`
/// entry carrying its media type and a content-derived `ETag`, ordered by key so
/// the generated file is reproducible.
///
/// The build script re-runs when any file below `dir` is added, changed or
/// removed. Symbolic links are skipped, so the table always matches the files
/// the manifest actually describes.
///
/// # Errors
///
/// Returns [`Error::NotADirectory`] when `dir` is missing or is not a directory,
/// [`Error::Io`] when an asset or the generated file cannot be handled, and
/// [`Error::MissingOutDir`] when called outside a build script.
pub fn embed_assets(dir: impl AsRef<Path>) -> Result<(), Error> {
    let declared = dir.as_ref();
    let resolved = scan::resolve(declared);

    // One traversal: its assets become the table, the paths it saw become the
    // watch list.
    let scan = scan::Scan::of(declared, &resolved)?;
    let generated = render::table(declared, &scan.assets);

    let target = output_path()?;
    std::fs::write(&target, generated).map_err(|source| Error::Io {
        operation: "write",
        path: target,
        source,
    })?;

    scan.announce_rerun(&resolved);
    Ok(())
}

/// Where [`embed_assets`] writes the generated table.
fn output_path() -> Result<std::path::PathBuf, Error> {
    let out_dir = std::env::var("OUT_DIR").map_err(|_| Error::MissingOutDir)?;
    Ok(Path::new(&out_dir).join(GENERATED_FILE))
}
