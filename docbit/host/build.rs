//! Compile the static site into the `docbit-host` binary.
//!
//! The tree lives at the `docbit/` level while this crate is `docbit/host/`, so
//! `../wwwroot` is the source of the compiled-in table. `build_host()` then
//! points `.use_spa("wwwroot")` at the *deployment* directory, which is where an
//! operator drops an override such as a replacement favicon.
//!
//! Returning the error rather than unwrapping keeps a missing or renamed
//! directory a build failure with the offending path, not a panic trace.

fn main() -> Result<(), rust_webx_build::Error> {
    rust_webx_build::embed_assets("../wwwroot")
}
