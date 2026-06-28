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

use crate::entities::{Category, Exhibition, Resource, Role, RoleUser, User};

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
    let now = 0i64; // 种子时间戳用 0，运行时由首次更新覆盖

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
    ctx.model().entity::<Category>().has_data(&[
        Category {
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
        },
        Category {
            id: 2,
            name: "ORM框架".into(),
            slug: "orm".into(),
            parent_id: None,
            sort_order: 1,
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            parent: BelongsTo::new(),
            children: HasMany::new(),
        },
        Category {
            id: 3,
            name: "Web框架".into(),
            slug: "framework".into(),
            parent_id: None,
            sort_order: 2,
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            parent: BelongsTo::new(),
            children: HasMany::new(),
        },
    ]);

    // 默认作品（与 docs/ 目录下的 INDEX.json 元数据一致）
    ctx.model().entity::<Exhibition>().has_data(&[
        Exhibition {
            id: 1,
            slug: "rust-ef".into(),
            title: "Rust Entity Framework".into(),
            subtitle: "接口导向 · EF Core 风格 · DI 集成 ORM".into(),
            description: "面向 Rust 开发者的 EF Core 风格 ORM 最佳实践指南，涵盖实体设计、LINQ 查询、变更跟踪、批量操作、事务迁移与 DI 集成。".into(),
            category_id: 2,  // orm
            tags: r#"["rust","orm","ef-core","database","linq"]"#.into(),
            repo_url: Some("https://gitcode.com/rf2026/rust-ef".into()),
            demo_url: None,
            docs_slug: Some("rust-ef".into()),
            featured: true,
            sort_order: 1,
            logo_url: Some("/assets/works/rust-ef.svg".into()),
            created_at: now,
            updated_at: now,
            created_id: None,
            updated_id: None,
            is_deleted: false,
            category: BelongsTo::new(),
        },
        Exhibition {
            id: 2,
            slug: "rust-webapp".into(),
            title: "Rust WebApplication Framework".into(),
            subtitle: "高内聚·编译时路由·DI+中介者双核心".into(),
            description: "生产级 Rust Web 服务框架，提供编译时路由扫描、零配置 Handler 注册、JWT 认证授权、统一异常中间件、事件发布/订阅及 SPA 托管能力。".into(),
            category_id: 3,  // framework
            tags: r#"["rust","webapp","webapi","mediator"]"#.into(),
            repo_url: Some("https://gitcode.com/rf2026/rust-webapp".into()),
            demo_url: None,
            docs_slug: Some("rust-webapp".into()),
            featured: true,
            sort_order: 2,
            logo_url: Some("/assets/works/rust-webapp.svg".into()),
            created_at: now,
            updated_at: now,
            created_id: None,
            updated_id: None,
            is_deleted: false,
            category: BelongsTo::new(),
        },
    ]);

    // 默认管理员账号（密码：admin123，bcrypt cost=4）
    ctx.model().entity::<User>().has_data(&[User {
        id: 1,
        name: "Administrator".into(),
        email: "admin@docbit.local".into(),
        password_hash: "$2b$04$0Txv1I1N9PmPg4I9fkbZUuFVeDWIDtmlD6CEjiwxAuLzSNMHVQ/3W".into(),
        created_id: None,
        created_at: now,
        updated_id: None,
        updated_at: now,
        is_deleted: false,
        roles: HasMany::new(),
    }]);

    // admin 用户关联 admin 角色
    ctx.model().entity::<RoleUser>().has_data(&[RoleUser {
        id: 1,
        user_id: 1,
        role_id: 1,
        created_at: now,
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
