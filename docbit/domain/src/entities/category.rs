//! Category entity — hierarchical via self-referencing `parent_id`.

use rust_ef::prelude::*;

#[derive(Debug, Clone, EntityType)]
#[table("categories")]
pub struct Category {
    #[primary_key]
    #[auto_increment]
    pub id: i32,
    #[required]
    #[max_length(100)]
    pub name: String,
    #[required]
    #[max_length(100)]
    #[unique]
    pub slug: String,
    #[foreign_key(Category)]
    pub parent_id: Option<i32>,
    #[required]
    pub sort_order: i32,
    #[required]
    pub created_at: i64,
    #[navigation]
    pub parent: BelongsTo<Category>,
    #[navigation]
    pub children: HasMany<Category>,
}
