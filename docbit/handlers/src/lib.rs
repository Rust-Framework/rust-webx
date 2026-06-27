//! Docbit handlers crate — service implementations and HTTP handlers.
//!
//! Each module contains `service.rs` (business logic) and `handler.rs`
//! (IRequestHandler implementations auto-registered via `#[rust_dicore::inject_attr]`).

pub mod auth;
pub mod blog;
pub mod cache;
pub mod category;
pub mod comment;
pub mod docs;
pub mod exhibition;
pub mod rbac;
pub mod site;
pub mod tracking;
pub mod user;
