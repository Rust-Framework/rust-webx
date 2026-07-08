//! UUID primary keys — application-assigned, no auto-increment.

use uuid::Uuid;

/// Generate a new random UUID v4 string for entity primary keys.
pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}

/// Stable UUIDs for bootstrap seed data (roles, categories, admin user, resources).
pub mod seed {
    pub const ROLE_ADMIN: &str = "00000000-0000-4000-8000-000000000001";
    pub const ROLE_USER: &str = "00000000-0000-4000-8000-000000000002";
    pub const CAT_UNCATEGORIZED: &str = "00000000-0000-4000-8000-000000000003";
    pub const CAT_ORM: &str = "00000000-0000-4000-8000-000000000004";
    pub const CAT_FRAMEWORK: &str = "00000000-0000-4000-8000-000000000005";
    pub const USER_ADMIN: &str = "00000000-0000-4000-8000-000000000010";
    pub const EXH_RUST_EF: &str = "00000000-0000-4000-8000-000000000011";
    pub const EXH_RUST_WEBX: &str = "00000000-0000-4000-8000-000000000012";
    pub const ROLE_USER_ADMIN: &str = "00000000-0000-4000-8000-000000000020";
    pub const RES_USERS_LIST: &str = "00000000-0000-4000-8000-000000000030";
    pub const RES_ROLES: &str = "00000000-0000-4000-8000-000000000031";
    pub const RES_RESOURCES: &str = "00000000-0000-4000-8000-000000000032";
    pub const RES_AUTHORIZES: &str = "00000000-0000-4000-8000-000000000033";
    pub const RES_TRACKING: &str = "00000000-0000-4000-8000-000000000034";
}
