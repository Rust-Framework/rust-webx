use serde::{Deserialize, Serialize};

/// User domain model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserModel {
    pub id: String,
    pub name: String,
    pub email: String,
    pub role: String,
    pub created_at: String,
}
