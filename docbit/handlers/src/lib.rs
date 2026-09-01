#![allow(non_snake_case)] // rust-dix #[derive(Inject)] generates __rdi_construct_* symbols

//! Docbit handlers crate — service implementations and HTTP handlers.
//!
//! Each module contains `IRequestHandler` implementations registered via
//! `#[derive(Inject)]` on the struct (generates `__rdi_construct_<Handler>`)
//! and `#[handler(inject)]` on the impl block (submits a `HandlerRegistration`
//! to `inventory` with a per-request factory + call bridge). The factory
//! resolves owned `DbContext` via `get_owned` each request, enabling
//! `handle(&mut self, ...)` without `Arc<Mutex>`.
//!
//! ## Handler 编写规范
//!
//! 每个 `handle` 按 **校验 → 加载 → 变更 → 持久化 → 响应** 分段，段间空一行。
//!
//! ### 写操作
//!
//! ```ignore
//! self.ctx.add(entity);
//!
//! crate::db::save_changes(&mut self.ctx).await?;
//! ```
//!
//! - Mutations go through `DbContext::add` / `update` / `attach`（rust-ef 1.8+）；不要对 `DbSet` 调用这些方法
//! - 禁止链式 `self.ctx.set::<T>().add(...)`（`set` 仅用于查询）
//!
//! Post-save reload: `ef_require_by_id!`（crate 根宏）
//!
//! ### 软删除与持久化
//!
//! - 软删除由 `docbit_domain::prepare_context` 全局过滤器处理，handler 禁止重复 `!is_deleted`
//! - 审计字段（`created_id`/`updated_id`）由 `docbit_domain` mapper 从 `RequestContext` 读取，handler 无需传 `op`
//! - 使用 `crate::db::{save_changes, EfResultExt}` 统一 ORM 错误映射

pub mod auth;
pub mod authorizer;
pub mod blog;
pub mod cache;
pub mod category;
pub mod comment;
pub mod db;
pub mod doc_service;
pub mod docs;
pub mod exhibition;
pub mod rbac;
pub mod site;
pub mod tracking;
pub mod user;
pub mod util;
