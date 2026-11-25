//! Docbit contracts crate — request types, read models, and service traits.
//!
//! One directory per context; inside it one file per operation plus `models.rs`
//! for the read models shared by more than one operation. This crate is the
//! innermost layer and must not depend on rust-ef.

pub mod auth;
pub mod blog;
pub mod cache;
pub mod category;
pub mod comment;
pub mod docs;
pub mod exhibition;
pub mod media;
pub mod rbac;
pub mod site;
pub mod tracking;
pub mod user;
