//! Seed data — default admin account only (inventory via CSV import).

use rust_ef::prelude::*;

use crate::entities::{Role, RoleUser, User};
use crate::ids::seed as id;

/// Argon2id hash for `admin123` (host default parameters).
const ADMIN_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$oPXAx056HRCWKUdW9W+Q9g$B7eyRVgvnDQoT4OFohCyZQempaBVjTD54UQ6nqx3nWw";

pub fn register(ctx: &mut DbContext) {
    let now = 0i64;

    ctx.model().entity::<Role>().has_data(&[Role {
        id: id::ROLE_ADMIN.into(),
        name: "admin".into(),
        description: "管理员".into(),
        created_id: None,
        created_at: now,
        updated_id: None,
        updated_at: now,
        is_deleted: false,
        users: HasMany::new(),
    }]);

    ctx.model().entity::<User>().has_data(&[User {
        id: id::USER_ADMIN.into(),
        name: "Administrator".into(),
        email: "admin@dmbit.local".into(),
        password_hash: ADMIN_PASSWORD_HASH.into(),
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
}
