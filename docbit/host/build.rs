//! Compile-time web root — which files are **baked into the binary**.
//!
//! This is not the runtime overlay path. At run time, `.use_spa("wwwroot")`
//! names the directory next to the exe where operators may drop overrides.
//! Pair with `#[webx::main(embed)]` so the generated table is linked in.

fn main() -> Result<(), webx::Error> {
    webx::builder().web_root("../wwwroot").build()
}
