//! Files compiled into the executable, read by path — `std::fs::read` for the
//! binary itself.
//!
//! ```ignore
//! let config = webx::assets::read_str("config/default.json");
//! let logo = webx::assets::read("images/logo.png");
//! ```
//!
//! Both read the table linked by `#[webx::main(embed)]` and return `None` when
//! the file is not embedded, or nothing is. Decoding, caching and path rules are
//! those of [`EmbeddedAssets::read`].

use crate::EmbeddedAssets;

/// The original contents of an embedded file.
pub fn read(path: &str) -> Option<&'static [u8]> {
    EmbeddedAssets::registered()?.read(path)
}

/// The contents of an embedded file as UTF-8 text; also `None` when it is not
/// UTF-8.
pub fn read_str(path: &str) -> Option<&'static str> {
    EmbeddedAssets::registered()?.read_str(path)
}
