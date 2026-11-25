//! Spec context — 设备规格 CRUD.

mod create_spec;
mod delete_spec;
mod get_spec;
mod list_product_specs;
mod list_specs;
pub mod queries;
mod update_spec;

pub use create_spec::CreateSpecHandler;
pub use delete_spec::DeleteSpecHandler;
pub use get_spec::GetSpecHandler;
pub use list_product_specs::ListProductSpecsHandler;
pub use list_specs::ListSpecsHandler;
pub use update_spec::UpdateSpecHandler;
