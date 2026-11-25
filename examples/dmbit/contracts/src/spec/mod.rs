//! Spec context — 设备规格 CRUD.

mod create_spec;
mod delete_spec;
mod get_spec;
mod list_product_specs;
mod list_specs;
pub mod models;
mod update_spec;

pub use create_spec::*;
pub use delete_spec::*;
pub use get_spec::*;
pub use list_product_specs::*;
pub use list_specs::*;
pub use models::*;
pub use update_spec::*;
