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
//! let set = self.ctx.set::<User>();
//! let entity = User { id: new_id(), /* ... */ };
//! set.add(entity);
//!
//! self.ctx
//!     .save_changes()
//!     .await
//!     .map_err(|e| Error::Internal(format!("Failed to create user: {}", e)))?;
//! ```
//!
//! - 禁止链式 `self.ctx.set::<T>().add(...)`
//! - 主键用 `new_id()` 预分配；同一业务操作只调用一次 `save_changes`
//!
//! ### 读操作
//!
//! - `linq!` 闭包捕获前先 `let q = ...`
//! - 按 id：`self.ctx.set::<T>().query().find(id).await?`

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
