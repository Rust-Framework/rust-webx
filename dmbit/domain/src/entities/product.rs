//! Product entity — master table.

use rust_ef::prelude::*;

use super::goods::Goods;

#[derive(Debug, Clone, EntityType)]
#[table("products")]
pub struct Product {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[max_length(100)]
    pub name: String,
    #[required]
    #[max_length(50)]
    #[unique]
    pub code: String,
    /// compute | storage
    #[required]
    #[max_length(20)]
    pub category: String,
    #[max_length(500)]
    pub remark: String,
    #[required]
    pub sort_order: i32,
    #[index]
    #[max_length(36)]
    pub created_id: Option<String>,
    #[required]
    pub created_at: i64,
    #[index]
    #[max_length(36)]
    pub updated_id: Option<String>,
    #[required]
    pub updated_at: i64,
    #[required]
    #[index]
    pub is_deleted: bool,
    #[navigation]
    pub goods: HasMany<Goods>,
}
