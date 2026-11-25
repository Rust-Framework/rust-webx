//! User ↔ `UserModel` mapping.

use dmbit_contracts::auth::models::UserModel;

use crate::entities::User;

impl From<&User> for UserModel {
    /// The principal as the API exposes it — roles included, hash excluded.
    ///
    /// Load the row with `include u.roles`; without the include `roles` is empty
    /// (the navigation is not lazily fetched by this mapper).
    fn from(user: &User) -> Self {
        UserModel {
            id: user.id.clone(),
            name: user.name.clone(),
            email: user.email.clone(),
            roles: user.roles.items().iter().map(|r| r.name.clone()).collect(),
            created_at: user.created_at,
        }
    }
}
