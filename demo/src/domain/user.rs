use serde::{Deserialize, Serialize};

/// User domain model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserModel {
    pub id: String,
    pub name: String,
    pub email: String,
    /// Bcrypt-hashed password. Empty for seed users with no password.
    #[serde(default)]
    pub password_hash: String,
    pub role: String,
    pub created_at: String,
}
