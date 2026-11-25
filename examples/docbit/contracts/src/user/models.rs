//! User read model — the admin-facing projection of a user account.
//!
//! `password_hash` is never part of this model; `roles` comes from the
//! multi-role `role_user` join.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserModel {
    pub id: String,
    pub name: String,
    pub email: String,
    pub roles: Vec<String>,
    pub created_at: i64,
}
