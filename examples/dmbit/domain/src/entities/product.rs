//! Product entity — master table.

use rust_ef::prelude::*;

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
    pub specs: HasMany<super::spec::Spec>,
}

/// Migration-safe model configuration for [`Product`].
///
/// Every column constraint is declared on the field above (one style for the
/// whole model); this type is the declared home for table-level rules, so a
/// future composite index or table option never turns into hand-written DDL.
#[derive(Debug, Default)]
pub struct ProductConfiguration;

#[rust_ef::entity(Product)]
impl IEntityTypeConfiguration<Product> for ProductConfiguration {
    fn configure(&self, _entity: &mut EntityTypeBuilder<'_, Product>) {}
}
