//! Auth read models shared by more than one operation.

use serde::Serialize;

/// The signed-in user projection is the same one the admin user API returns;
/// re-exported here so `AuthResponse` and the user context stay one type.
pub use crate::user::UserModel;

/// Registration / login response: the minted JWT plus the identity it describes.
#[derive(Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: UserModel,
}
