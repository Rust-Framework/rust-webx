//! Product context — 产品（机型）主数据 CRUD.

mod create_product;
mod delete_product;
mod get_product;
mod list_products;
pub mod queries;
mod update_product;

pub use create_product::CreateProductHandler;
pub use delete_product::DeleteProductHandler;
pub use get_product::GetProductHandler;
pub use list_products::ListProductsHandler;
pub use update_product::UpdateProductHandler;
