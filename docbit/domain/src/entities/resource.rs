//! Resource + Authorize entities — RBAC dynamic authorization matrix.

use rust_ef::prelude::*;

use super::role::Role;

#[derive(Debug, Clone, EntityType)]
#[table("resources")]
pub struct Resource {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[max_length(100)]
    pub name: String,
    #[max_length(500)]
    pub description: String,
    #[required]
    #[max_length(20)]
    #[index]
    #[column("type")]
    pub resource_type: String,
    #[required]
    #[max_length(200)]
    pub value: String,
    #[required]
    pub properties: String,
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
    pub roles: HasMany<Role, Authorize>,
}

#[derive(Debug, Clone, EntityType)]
#[table("authorizes")]
pub struct Authorize {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[foreign_key(Role)]
    #[index]
    #[max_length(36)]
    pub role_id: String,
    #[required]
    #[foreign_key(Resource)]
    #[index]
    #[max_length(36)]
    pub resource_id: String,
    #[required]
    pub created_at: i64,
}
