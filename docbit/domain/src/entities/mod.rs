//! Domain entities — simplified naming (no `Entity` suffix).
//!
//! All entities use rust-ef 1.1.0 patterns:
//! - `#[derive(Debug, Clone, EntityType, Serialize, Deserialize)]`
//! - `#[table("name")]` for table mapping
//! - `#[primary_key]` + `#[auto_increment]` for i32 PKs
//! - `#[foreign_key(TargetEntity)]` for FKs
//! - `#[navigation]` + `BelongsTo<T>` / `HasMany<T>` / `HasMany<T, Through>`

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
