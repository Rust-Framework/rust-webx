//! UUID primary keys — application-assigned.

use uuid::Uuid;

pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}

pub mod seed {
    pub const ROLE_ADMIN: &str = "00000000-0000-4000-8000-000000000001";
    pub const USER_ADMIN: &str = "00000000-0000-4000-8000-000000000010";
    pub const ROLE_USER_ADMIN: &str = "00000000-0000-4000-8000-000000000020";
}
