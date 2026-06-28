//! Docbit handlers crate — service implementations and HTTP handlers.
//!
//! Each module contains `IRequestHandler` implementations auto-registered via
//! `#[rust_dicore::inject_attr]` + `#[handler(inject)]`. 业务服务（IBlogService）
//! 与文档服务（IDocumentService 由 host 提供）按需注入。

pub mod auth;
pub mod authorizer;
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
pub mod util;
