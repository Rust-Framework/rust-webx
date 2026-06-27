//! Domain entities — simplified naming (no `Entity` suffix).
//!
//! All entities use rust-ef 1.1.0 patterns:
//! - `#[derive(Debug, Clone, EntityType)]`（不加 Serialize/Deserialize，导航字段未实现这些 trait）
//! - `#[table("name")]` for table mapping
//! - `#[primary_key]` + `#[auto_increment]` for i32 PKs
//! - `#[foreign_key(TargetEntity)]` for FKs
//! - `#[navigation]` + `BelongsTo<T>` / `HasMany<T>` / `HasMany<T, Through>`
//! - 主表统一追加审计字段（created_id/created_at/updated_id/updated_at/is_deleted）；
//!   created_id/updated_id 用 `Option<i32>` + `#[index]`，不加 `#[foreign_key]`。

pub mod blog;
pub mod category;
pub mod comment;
pub mod exhibition;
pub mod password_reset;
pub mod resource;
pub mod role;
pub mod tracking;
pub mod user;

pub use blog::*;
pub use category::*;
pub use comment::*;
pub use exhibition::*;
pub use password_reset::*;
pub use resource::*;
pub use role::*;
pub use tracking::*;
pub use user::*;
