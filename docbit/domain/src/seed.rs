//! Seed data — default roles, categories, and core resources.
//!
//! Called before `ensure_created()` so the framework inserts seed rows after
//! table creation. The admin user (which needs a bcrypt password hash) is
//! created separately in the host's `DbInitService` startup step, since
//! bcrypt hashing is a runtime concern.
//!
//! API (rust-ef 1.1.0):
//! ```
//! ctx.model().entity::<T>().has_data(&[ ... ]);
//! ctx.ensure_created().await?;
//! ```

use rust_ef::prelude::*;

use crate::entities::{Category, Resource, Role};

/// 资源分类常量
pub const RES_TYPE_APP: &str = "应用";
pub const RES_TYPE_MODULE: &str = "模块";
pub const RES_TYPE_PAGE: &str = "页面";
pub const RES_TYPE_ACTION: &str = "操作";
pub const RES_TYPE_DATA: &str = "数据";
pub const RES_TYPE_OTHER: &str = "其他";

/// Register seed data on the DbContext. Call `ctx.ensure_created().await?`
/// afterwards to create tables and insert the seed rows.
pub fn seed(ctx: &mut DbContext) {
    let now = 0i64; // 种子时间戳用 0，运行时由 AuditInterceptor 在实际写入时覆盖

    // 默认角色：admin + user
    ctx.model().entity::<Role>().has_data(&[
        Role {
            id: 1,
            name: "admin".into(),
            description: "管理员".into(),
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            users: HasMany::new(),
            resources: HasMany::new(),
        },
        Role {
            id: 2,
            name: "user".into(),
            description: "普通用户".into(),
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            users: HasMany::new(),
            resources: HasMany::new(),
        },
    ]);

    // 默认根分类
    ctx.model().entity::<Category>().has_data(&[Category {
        id: 1,
        name: "未分类".into(),
        slug: "uncategorized".into(),
        parent_id: None,
        sort_order: 0,
        created_id: None,
        created_at: now,
        updated_id: None,
        updated_at: now,
        is_deleted: false,
        parent: BelongsTo::new(),
        children: HasMany::new(),
    }]);

    // 核心操作资源（动态鉴权用）：admin 专属管理接口
    ctx.model().entity::<Resource>().has_data(&[
        Resource {
            id: 1,
            name: "用户管理-列表".into(),
            description: "查看用户列表".into(),
            resource_type: RES_TYPE_ACTION.into(),
            value: "/api/users".into(),
            properties: r#"{"method":"GET"}"#.into(),
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            roles: HasMany::new(),
        },
        Resource {
            id: 2,
            name: "角色管理".into(),
            description: "角色 CRUD".into(),
            resource_type: RES_TYPE_ACTION.into(),
            value: "/api/roles/*".into(),
            properties: r#"{"method":"*"}"#.into(),
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            roles: HasMany::new(),
        },
        Resource {
            id: 3,
            name: "资源管理".into(),
            description: "权限资源 CRUD".into(),
            resource_type: RES_TYPE_ACTION.into(),
            value: "/api/resources/*".into(),
            properties: r#"{"method":"*"}"#.into(),
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            roles: HasMany::new(),
        },
        Resource {
            id: 4,
            name: "授权管理".into(),
            description: "角色-资源授权 CRUD".into(),
            resource_type: RES_TYPE_ACTION.into(),
            value: "/api/authorizes/*".into(),
            properties: r#"{"method":"*"}"#.into(),
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            roles: HasMany::new(),
        },
        Resource {
            id: 5,
            name: "访问统计".into(),
            description: "站点访问统计查询".into(),
            resource_type: RES_TYPE_ACTION.into(),
            value: "/api/tracking/*".into(),
            properties: r#"{"method":"GET"}"#.into(),
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            roles: HasMany::new(),
        },
    ]);
}
