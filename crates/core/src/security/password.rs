//! Password hashing port ([`IPasswordHasher`]).
//!
//! Applications hash and verify passwords through this port instead of carrying
//! their own Argon2 (or bcrypt, or scrypt) code. Hashes are **PHC strings** —
//! the self-describing `$argon2id$v=19$m=…,t=…,p=…$salt$hash` format — so the
//! algorithm and its parameters travel with the stored value and can be upgraded
//! without a schema change.
//!
//! The default implementation is `Argon2PasswordHasher` in `rust-webx-host`
//! (feature `password-argon2`, on by default); it runs Argon2 on a blocking
//! thread so a login does not stall the async runtime. Register a different
//! implementation in DI to replace it — handlers keep injecting
//! `Arc<dyn IPasswordHasher>` and never need to change.

use crate::error::Result;

/// Hash and verify passwords (Argon2id by default).
///
/// ```ignore
/// let phc = hasher.hash(&password).await?;             // login / registration
/// let ok = hasher.verify(&password, &stored.phc).await?; // 401 on false
/// if ok && hasher.needs_rehash(&stored.phc) {
///     stored.phc = hasher.hash(&password).await?;      // transparent upgrade
/// }
/// ```
#[async_trait::async_trait]
pub trait IPasswordHasher: Send + Sync {
    /// Hash one password with a fresh random salt; returns a PHC string.
    ///
    /// The implementation must not panic on any input; failures (including
    /// misconfigured parameters) come back as a typed
    /// [`Error`](crate::error::Error).
    async fn hash(&self, password: &str) -> Result<String>;

    /// Verify `password` against a stored PHC hash.
    ///
    /// * `Ok(true)`  — the password matches.
    /// * `Ok(false)` — the password does not match (a client error, not a server
    ///   error: do not log the stored hash or the password).
    /// * `Err(...)`  — the stored hash is malformed, uses an unsupported
    ///   algorithm, or the hashing backend failed. This is **server-side** data
    ///   or configuration corruption; reporting it as `Ok(false)` would silently
    ///   turn a broken account into "wrong password".
    async fn verify(&self, password: &str, stored_hash: &str) -> Result<bool>;

    /// Whether a stored hash was produced with weaker/other parameters than the
    /// ones this hasher currently uses.
    ///
    /// Call after a successful [`IPasswordHasher::verify`] and re-hash the
    /// password when it returns `true` — this is how cost parameters are raised
    /// without a migration. A hash that cannot be parsed, or that uses a
    /// different algorithm, also returns `true` (it cannot be trusted as-is).
    fn needs_rehash(&self, stored_hash: &str) -> bool;
}
