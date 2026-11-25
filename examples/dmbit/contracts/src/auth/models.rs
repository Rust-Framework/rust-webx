//! Auth read models.

use serde::Serialize;

/// The signed-in principal as the API exposes it.
///
/// The naming chain is `User` (entity) ↔ `UserModel` (read model); the password
/// hash never leaves the domain layer.
#[derive(Debug, Clone, Serialize)]
pub struct UserModel {
    pub id: String,
    pub name: String,
    pub email: String,
    pub roles: Vec<String>,
    pub created_at: i64,
}
