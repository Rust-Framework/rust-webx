//! Docbit handlers crate — service implementations and HTTP handlers.
//!
//! Each module contains `IRequestHandler` implementations registered via
//! `#[derive(Inject)]` on the struct (generates `__rdi_construct_<Handler>`)
//! and `#[handler(inject)]` on the impl block (submits a `HandlerRegistration`
//! to `inventory` with a per-request factory + call bridge). The factory
//! resolves owned `DbContext` via `get_owned` each request, enabling
//! `handle(&mut self, ...)` without `Arc<Mutex>`.
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
