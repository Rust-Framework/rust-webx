//! Runtime access to the table a build script registers.
//!
//! The generated `webx_embedded_assets.rs` ends in an `inventory::submit!`;
//! this binary registers a table the same way, so the lookup is exercised
//! end to end rather than against a hand-built value.

use webx_spa::{inventory, ContentEncoding, EmbeddedAsset, EmbeddedAssets};

const FILES: &[EmbeddedAsset] = &[EmbeddedAsset {
    path: "config/default.json",
    bytes: br#"{"name":"webx"}"#,
    raw_len: 15,
    encoding: ContentEncoding::Identity,
    content_type: "application/json",
    etag: "\"sha256-cfg\"",
}];

inventory::submit! {
    EmbeddedAssets::new("wwwroot", FILES)
}

#[test]
fn the_registered_table_is_readable_without_a_host() {
    let assets = EmbeddedAssets::registered().expect("a table is registered");
    assert_eq!(assets.root(), "wwwroot");
    assert_eq!(
        assets.read_str("config/default.json"),
        Some(r#"{"name":"webx"}"#)
    );
}

#[test]
fn the_free_functions_read_the_registered_table() {
    use webx_spa::assets;

    assert_eq!(
        assets::read_str("/config/default.json"),
        Some(r#"{"name":"webx"}"#)
    );
    assert_eq!(assets::read("config/default.json"), Some(FILES[0].bytes));
    assert!(assets::read("missing.txt").is_none());
}
