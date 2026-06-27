//! Resource + Authorize entities — RBAC dynamic authorization matrix.
//!
//! `Resource` records a route pattern + HTTP method; `Authorize` is the join
//! table between `Role` and `Resource` (many-to-many).

use rust_ef::prelude::*;

use super::role::Role;

#[derive(Debug, Clone, EntityType)]
#[table("resources")]
pub struct Resource {
    #[primary_key]
    #[auto_increment]
    pub id: i32,
    #[required]
    #[max_length(200)]
    #[unique]
    pub route_pattern: String, // 如 /api/blog/{slug}、/api/users/*
    #[required]
    #[max_length(10)]
    pub method: String, // GET/POST/PUT/DELETE/*
    #[max_length(200)]
    pub description: String,
    #[required]
    pub created_at: i64,
    #[navigation]
    pub roles: HasMany<Role, Authorize>,
}

#[derive(Debug, Clone, EntityType)]
#[table("authorizes")]
pub struct Authorize {
    #[primary_key]
    #[auto_increment]
    pub id: i32,
    #[required]
    #[foreign_key(Role)]
    #[index]
    pub role_id: i32,
    #[required]
    #[foreign_key(Resource)]
    #[index]
    pub resource_id: i32,
}
