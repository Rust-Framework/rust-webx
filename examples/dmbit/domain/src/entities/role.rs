//! Role + RoleUser entities.

use rust_ef::prelude::*;

use super::user::User;

#[derive(Debug, Clone, EntityType)]
#[table("roles")]
pub struct Role {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[max_length(50)]
    #[unique]
    pub name: String,
    #[max_length(200)]
    pub description: String,
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
    pub users: HasMany<User, RoleUser>,
}

/// Migration-safe model configuration for [`Role`].
#[derive(Debug, Default)]
pub struct RoleConfiguration;

#[rust_ef::entity(Role)]
impl IEntityTypeConfiguration<Role> for RoleConfiguration {
    fn configure(&self, _entity: &mut EntityTypeBuilder<'_, Role>) {}
}

#[derive(Debug, Clone, EntityType)]
#[table("role_users")]
pub struct RoleUser {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[foreign_key(User)]
    #[index]
    #[max_length(36)]
    pub user_id: String,
    #[required]
    #[foreign_key(Role)]
    #[index]
    #[max_length(36)]
    pub role_id: String,
    #[required]
    pub created_at: i64,
}

/// Migration-safe model configuration for [`RoleUser`].
#[derive(Debug, Default)]
pub struct RoleUserConfiguration;

#[rust_ef::entity(RoleUser)]
impl IEntityTypeConfiguration<RoleUser> for RoleUserConfiguration {
    /// One row per (user, role): a duplicate grant must fail at the database,
    /// not in a `find-then-insert` race. Declared here (never as hand-written
    /// `CREATE INDEX`) and materialized by the official migration tooling.
    fn configure(&self, entity: &mut EntityTypeBuilder<'_, RoleUser>) {
        entity
            .has_index(&[RoleUser::COLUMN_USER_ID, RoleUser::COLUMN_ROLE_ID])
            .is_unique();
    }
}
