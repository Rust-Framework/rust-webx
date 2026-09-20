//! OpenAPI 3.0 specification generation and API docs UI (package
//! `rust-webx-openapi`, imported as `webx_openapi`).
//!
//! Builds the spec from compile-time route metadata and provides the embedded
//! API documentation page (`APIUI_HTML`) served by the host crate.

mod apiui;
mod openapi;

pub use apiui::APIUI_HTML;
pub use openapi::generate_openapi_spec;
