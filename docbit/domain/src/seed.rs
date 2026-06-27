//! Seed data — default roles and categories.
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

use crate::entities::{Category, Role};

/// Register seed data on the DbContext. Call `ctx.ensure_created().await?`
/// afterwards to create tables and insert the seed rows.
pub fn seed(ctx: &mut DbContext) {
    // 默认角色：admin + user
    ctx.model().entity::<Role>().has_data(&[
        Role {
            id: 1,
            name: "admin".into(),
            description: "管理员".into(),
            users: HasMany::new(),
            resources: HasMany::new(),
        },
        Role {
            id: 2,
            name: "user".into(),
            description: "普通用户".into(),
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
        created_at: 0,
        parent: BelongsTo::new(),
        children: HasMany::new(),
    }]);
}
