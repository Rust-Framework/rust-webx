//! Resource + Authorize entities — RBAC dynamic authorization matrix.
//!
//! `Resource` 采用通用 `type + value + properties` 三段式模型：
//! - type：应用/模块/页面/操作/数据/其他
//! - value：页面/操作类型时为路由（如 `/api/blog/{slug}`）
//! - properties：JSON 配置，操作类型存 `{"method":"GET"}` 等
//! `Authorize` 是 `Role` ↔ `Resource` 的联结表（多对多）。

use rust_ef::prelude::*;

use super::role::Role;

#[derive(Debug, Clone, EntityType)]
#[table("resources")]
pub struct Resource {
    #[primary_key]
    #[auto_increment]
    pub id: i32,
    #[required]
    #[max_length(100)]
    pub name: String, // 资源名称
    #[max_length(500)]
    pub description: String, // 资源描述
    #[required]
    #[max_length(20)]
    #[index]
    #[column("type")]
    pub resource_type: String, // 资源分类：应用/模块/页面/操作/数据/其他（DB 列名 type）
    #[required]
    #[max_length(200)]
    pub value: String, // 资源值；页面/操作时为路由
    #[required]
    pub properties: String, // 配置属性 JSON，如 {"method":"GET"}
    #[index]
    pub created_id: Option<i32>, // 无 FK
    #[required]
    pub created_at: i64,
    #[index]
    pub updated_id: Option<i32>, // 无 FK
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
    #[required]
    pub created_at: i64, // 联结表仅留 created_at
}
