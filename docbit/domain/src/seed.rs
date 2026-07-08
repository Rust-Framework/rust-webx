//! Seed data — default roles, categories, and core resources (stable UUID PKs).

use rust_ef::prelude::*;

use crate::entities::{Category, Exhibition, Resource, Role, RoleUser, User};
use crate::ids::seed as id;

/// 资源分类常量
pub const RES_TYPE_APP: &str = "应用";
pub const RES_TYPE_MODULE: &str = "模块";
pub const RES_TYPE_PAGE: &str = "页面";
pub const RES_TYPE_ACTION: &str = "操作";
pub const RES_TYPE_DATA: &str = "数据";
pub const RES_TYPE_OTHER: &str = "其他";

/// Register seed data on the DbContext.
pub fn seed(ctx: &mut DbContext) {
    let now = 0i64;

    ctx.model().entity::<Role>().has_data(&[
        Role {
            id: id::ROLE_ADMIN.into(),
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
            id: id::ROLE_USER.into(),
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

    ctx.model().entity::<Category>().has_data(&[
        Category {
            id: id::CAT_UNCATEGORIZED.into(),
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
            id: id::CAT_ORM.into(),
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
            id: id::CAT_FRAMEWORK.into(),
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

    ctx.model().entity::<Exhibition>().has_data(&[
        Exhibition {
            id: id::EXH_RUST_EF.into(),
            slug: "rust-ef".into(),
            title: "Rust Entity Framework".into(),
            subtitle: "接口导向 · EF Core 风格 · DI 集成 ORM".into(),
            description: "面向 Rust 开发者的 EF Core 风格 ORM 最佳实践指南，涵盖实体设计、LINQ 查询、变更跟踪、批量操作、事务迁移与 DI 集成。".into(),
            category_id: id::CAT_ORM.into(),
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
            id: id::EXH_RUST_WEBX.into(),
            slug: "rust-webx".into(),
            title: "Rust WebApplication Framework".into(),
            subtitle: "高内聚·编译时路由·DI+中介者双核心".into(),
            description: "生产级 Rust Web 服务框架，提供编译时路由扫描、零配置 Handler 注册、JWT 认证授权、统一异常中间件、事件发布/订阅及 SPA 托管能力。".into(),
            category_id: id::CAT_FRAMEWORK.into(),
            tags: r#"["rust","webapp","webapi","mediator"]"#.into(),
            repo_url: Some("https://gitcode.com/rf2026/rust-webx".into()),
            demo_url: None,
            docs_slug: Some("rust-webx".into()),
            featured: true,
            sort_order: 2,
            logo_url: Some("/assets/works/rust-webx.svg".into()),
            created_at: now,
            updated_at: now,
            created_id: None,
            updated_id: None,
            is_deleted: false,
            category: BelongsTo::new(),
        },
    ]);

    ctx.model().entity::<User>().has_data(&[User {
        id: id::USER_ADMIN.into(),
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

    ctx.model().entity::<RoleUser>().has_data(&[RoleUser {
        id: id::ROLE_USER_ADMIN.into(),
        user_id: id::USER_ADMIN.into(),
        role_id: id::ROLE_ADMIN.into(),
        created_at: now,
    }]);

    ctx.model().entity::<Resource>().has_data(&[
        Resource {
            id: id::RES_USERS_LIST.into(),
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
            id: id::RES_ROLES.into(),
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
            id: id::RES_RESOURCES.into(),
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
            id: id::RES_AUTHORIZES.into(),
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
            id: id::RES_TRACKING.into(),
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

