//! Session revocation port ([`ISessionStore`]).
//!
//! A token stays signature-valid until `exp`. Logout and forced sign-out need a
//! server-side record the authentication middleware can consult after verifying
//! the token; that record is keyed by the token's `jti` claim.

use crate::error::{Error, Result};
use std::collections::HashSet;
use std::sync::{PoisonError, RwLock};

/// Revocation state for issued tokens.
///
/// The authentication middleware calls [`ISessionStore::is_revoked`] with the
/// `jti` of an already verified token; `true` turns the request into a 401
/// without waiting for the token to expire. Implementations back this with
/// whatever survives their deployment — process memory, a cache with TTL, or a
/// database table.
///
/// Errors are not swallowed: a store failure fails the request (the middleware
/// must not authorize when it cannot prove the token is unrevoked).
#[async_trait::async_trait]
pub trait ISessionStore: Send + Sync {
    /// Revoke one token by its `jti` (logout of a single session).
    ///
    /// Revoking an already-revoked or unknown `jti` is not an error: logout is
    /// idempotent.
    async fn revoke(&self, jti: &str) -> Result<()>;

    /// Whether the token `jti` has been revoked.
    async fn is_revoked(&self, jti: &str) -> Result<bool>;

    /// Revoke every token of `subject` ("sign out everywhere").
    ///
    /// Optional capability: the default implementation returns a typed
    /// [`Error::State`] instead of silently succeeding, because a store that
    /// cannot enumerate a subject's sessions would otherwise report a logout
    /// that did not happen. Override it when the backing store keeps a
    /// subject-to-session index.
    async fn revoke_all_for_subject(&self, subject: &str) -> Result<()> {
        Err(Error::State(format!(
            "this ISessionStore cannot revoke every token of '{subject}'; override revoke_all_for_subject"
        )))
    }
}

/// Process-local [`ISessionStore`] — the dependency-free default.
///
/// Suitable for a single-process deployment, tests and development. Two
/// limitations are deliberate and documented rather than hidden:
///
/// * Revocations die with the process (a restart resurrects every token that
///   has not expired yet) — use a persistent store in production when that
///   matters.
/// * Revoked `jti`s are never garbage-collected (the store cannot know when a
///   token expires), so a long-running process accumulates one `String` per
///   revoked token.
/// * [`ISessionStore::revoke_all_for_subject`] is not supported: the store keeps
///   no subject index.
#[derive(Debug, Default)]
pub struct InMemorySessionStore {
    revoked: RwLock<HashSet<String>>,
}

impl InMemorySessionStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of revoked token ids currently tracked (diagnostics/tests).
    pub fn revoked_count(&self) -> usize {
        self.revoked
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

#[async_trait::async_trait]
impl ISessionStore for InMemorySessionStore {
    async fn revoke(&self, jti: &str) -> Result<()> {
        if jti.is_empty() {
            // An empty `jti` never identifies a token: storing it would revoke
            // nothing while reporting success.
            return Err(Error::Validation("jti must not be empty".to_string()));
        }
        self.revoked
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(jti.to_string());
        Ok(())
    }

    async fn is_revoked(&self, jti: &str) -> Result<bool> {
        Ok(self
            .revoked
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(jti))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn revoke_marks_only_that_token() {
        let store = InMemorySessionStore::new();
        assert!(!store.is_revoked("jti-a").await.unwrap());

        store.revoke("jti-a").await.unwrap();

        assert!(store.is_revoked("jti-a").await.unwrap());
        assert!(
            !store.is_revoked("jti-b").await.unwrap(),
            "revocation is per token, not global"
        );
        assert_eq!(store.revoked_count(), 1);
    }

    #[tokio::test]
    async fn revoke_is_idempotent() {
        let store = InMemorySessionStore::new();
        store.revoke("jti-a").await.unwrap();
        store.revoke("jti-a").await.unwrap();
        assert_eq!(store.revoked_count(), 1, "logout must be idempotent");
    }

    #[tokio::test]
    async fn empty_jti_is_a_typed_error() {
        let err = InMemorySessionStore::new().revoke("").await.unwrap_err();
        assert!(matches!(err, Error::Validation(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn revoke_all_for_subject_is_unsupported_by_default() {
        let err = InMemorySessionStore::new()
            .revoke_all_for_subject("user-1")
            .await
            .expect_err("the dependency-free store has no subject index");
        assert!(matches!(err, Error::State(_)), "got {err:?}");
    }
}
