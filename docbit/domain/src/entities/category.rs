//! Category entity — hierarchical via self-referencing `parent_id`.

use rust_ef::prelude::*;

#[derive(Debug, Clone, EntityType)]
#[table("categories")]
pub struct Category {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[max_length(100)]
    pub name: String,
    #[required]
    #[max_length(100)]
    #[unique]
    pub slug: String,
    #[foreign_key(Category)]
    #[max_length(36)]
    pub parent_id: Option<String>,
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
    pub parent: BelongsTo<Category>,
    #[navigation]
    pub children: HasMany<Category>,
}
