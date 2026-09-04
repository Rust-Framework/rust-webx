//! Seed data — default admin account only (inventory via CSV import).

use rust_ef::prelude::*;

use crate::entities::{Role, RoleUser, User};
use crate::ids::seed as id;

/// bcrypt hash for `admin123` (cost 4, same as docbit seed).
const ADMIN_PASSWORD_HASH: &str = "$2b$04$0Txv1I1N9PmPg4I9fkbZUuFVeDWIDtmlD6CEjiwxAuLzSNMHVQ/3W";

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
