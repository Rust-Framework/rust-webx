//! Docbit handlers crate — service implementations and HTTP handlers.
//!
//! Each module contains `IRequestHandler` implementations auto-registered via
//! `#[rust_dicore::inject]` (on struct, generates constructor) +
//! `#[handler(inject)]` (on impl, registers in LRWF HandlerCache).
//! 业务服务（`IDocumentService` 由本 crate 的 `doc_service` 提供）按需注入。

pub mod auth;
pub mod authorizer;
pub mod blog;
pub mod cache;
pub mod category;
pub mod comment;
pub mod doc_service;
pub mod docs;
pub mod exhibition;
pub mod rbac;
pub mod site;
pub mod tracking;
pub mod user;
pub mod util;
