//! Registered user aggregate.

/// A user and the credential hash the security port verifies.
///
/// The hash never leaves this type: handlers receive `(user_id, hash)` only
/// through [`ImStore::credentials`](crate::ImStore::credentials), and the
/// framework `IPasswordHasher` does the verifying.
#[derive(Debug, Clone)]
pub struct User {
    id: String,
    username: String,
    password_hash: String,
}

impl User {
    pub fn new(
        id: impl Into<String>,
        username: impl Into<String>,
        password_hash: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            username: username.into(),
            password_hash: password_hash.into(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn password_hash(&self) -> &str {
        &self.password_hash
    }
}
