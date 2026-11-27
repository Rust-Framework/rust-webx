//! Session revocation port ([`ISessionStore`]).
//!
//! A token stays signature-valid until `exp`. Logout and forced sign-out need a
//! server-side record the authentication middleware can consult after verifying
//! the token; that record is keyed by the token's `jti` claim.
//!
//! The record carries the token expiry when the caller knows it
//! ([`ISessionStore::revoke_until`]), so a long-running store does not keep dead
//! `jti`s forever. The default `revoke` remains indefinite for compatibility
//! with stores that only know how to insert a key.

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

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

    /// Revoke `jti` until `expires_at_unix_secs` (normally the token's `exp`).
    ///
    /// Once the token expiry has passed the revocation record is no longer
    /// needed: the token is invalid by its own `exp` claim. Implementations
    /// should delete the record after that instant.
    ///
    /// The default delegates to [`revoke`](Self::revoke) and keeps the record
    /// forever — override it when the backing store can express a TTL.
    async fn revoke_until(&self, jti: &str, expires_at_unix_secs: u64) -> Result<()> {
        let _ = expires_at_unix_secs;
        self.revoke(jti).await
    }

    /// Whether the token `jti` has been revoked.
    async fn is_revoked(&self, jti: &str) -> Result<bool>;

    /// Revoke every token of `subject` ("sign out everywhere").
    ///
    /// Optional capability: the default implementation returns
    /// `501 Not Implemented` ([`Error::status`]) instead of silently succeeding,
    /// because a store that cannot enumerate a subject's sessions would
    /// otherwise report a logout that did not happen. `501` lets the caller (and
    /// the logs) tell "not supported" from a real server failure. Override it
    /// when the backing store keeps a subject-to-session index.
    async fn revoke_all_for_subject(&self, subject: &str) -> Result<()> {
        Err(Error::status(
            501,
            format!(
                "this ISessionStore cannot revoke every token of '{subject}'; override revoke_all_for_subject"
            ),
        ))
    }

    /// Whether this store supports revoking sessions by refresh family
    /// ([`revoke_by_refresh_family`](Self::revoke_by_refresh_family)).
    ///
    /// Explicit capability declaration: do not infer it from the error code of
    /// `revoke_by_refresh_family`, otherwise a real backend failure that happens
    /// to map to the same status would be mistaken for "not supported".
    fn supports_refresh_family_revocation(&self) -> bool {
        false
    }

    /// Revoke every session bound to `refresh_family_id`; returns how many were
    /// revoked.
    ///
    /// This is the **paired revocation** after refresh-token reuse: the refresh
    /// family is already dead, but the access tokens minted from it stay
    /// signature-valid until their own `exp`. A store that keeps a
    /// `refresh_family_id` index can turn each of those sessions into a 401
    /// immediately.
    ///
    /// Optional capability: the default implementation returns
    /// `501 Not Implemented` ([`Error::status`]) instead of silently succeeding,
    /// because a store without the index would otherwise report a completed
    /// paired revocation when it revoked nothing. Override it when the backing
    /// store keeps a family-to-session index — and override
    /// [`supports_refresh_family_revocation`](Self::supports_refresh_family_revocation)
    /// to declare that capability: callers decide whether to skip the session
    /// half from that declaration, not from this error, because a real backend
    /// failure can be reported with the same `501` status.
    async fn revoke_by_refresh_family(&self, refresh_family_id: &str) -> Result<u64> {
        Err(Error::status(
            501,
            format!(
                "this ISessionStore cannot revoke sessions of refresh family '{refresh_family_id}'; override revoke_by_refresh_family"
            ),
        ))
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
/// * [`revoke`](ISessionStore::revoke) without an expiry is indefinite. Use
///   [`revoke_until`](ISessionStore::revoke_until) with the token's `exp` (and
///   [`purge_expired`](InMemorySessionStore::purge_expired) as a sweep) to keep
///   the map bounded.
/// * [`ISessionStore::revoke_all_for_subject`] and
///   [`ISessionStore::revoke_by_refresh_family`] are not supported: the store
///   keeps neither a subject index nor a refresh-family index, so
///   [`supports_refresh_family_revocation`](ISessionStore::supports_refresh_family_revocation)
///   stays `false`.
#[derive(Debug, Default)]
pub struct InMemorySessionStore {
    /// `jti` → absolute expiry (Unix seconds); `0` means "no expiry".
    revoked: RwLock<HashMap<String, u64>>,
}

impl InMemorySessionStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of tracked `jti`s (including expired entries not yet purged).
    pub fn revoked_count(&self) -> usize {
        self.revoked
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Remove every record whose token expiry has passed; returns the count.
    ///
    /// [`is_revoked`](ISessionStore::is_revoked) also removes an expired record
    /// on lookup; this sweep handles ids that are never looked up again.
    pub fn purge_expired(&self, now_unix_secs: u64) -> usize {
        let mut revoked = self.revoked.write().unwrap_or_else(PoisonError::into_inner);
        let before = revoked.len();
        revoked.retain(|_, expires_at| *expires_at == 0 || *expires_at > now_unix_secs);
        before - revoked.len()
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

#[async_trait::async_trait]
impl ISessionStore for InMemorySessionStore {
    async fn revoke(&self, jti: &str) -> Result<()> {
        if jti.is_empty() {
            // An empty `jti` never identifies a token: storing it would revoke
            // nothing while reporting success.
            return Err(Error::validation("jti must not be empty".to_string()));
        }
        self.revoked
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(jti.to_string(), 0);
        Ok(())
    }

    async fn revoke_until(&self, jti: &str, expires_at_unix_secs: u64) -> Result<()> {
        if jti.is_empty() {
            return Err(Error::validation("jti must not be empty".to_string()));
        }
        // A token that is already past `exp` does not need a revocation record.
        if expires_at_unix_secs != 0 && expires_at_unix_secs <= now_unix_secs() {
            return Ok(());
        }
        self.revoked
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(jti.to_string(), expires_at_unix_secs);
        Ok(())
    }

    async fn is_revoked(&self, jti: &str) -> Result<bool> {
        let now = now_unix_secs();
        let expires_at = self
            .revoked
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(jti)
            .copied();

        match expires_at {
            Some(0) => Ok(true),
            Some(expires_at) if expires_at > now => Ok(true),
            Some(_) => {
                // Lazy delete: the token's own `exp` has passed, so the record
                // can never matter again.
                self.revoked
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(jti);
                Ok(false)
            }
            None => Ok(false),
        }
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
        let store = InMemorySessionStore::new();
        let err = store.revoke("").await.unwrap_err();
        assert!(err.code() == "validation_failed", "got {err:?}");
        let err = store.revoke_until("", 10).await.unwrap_err();
        assert!(err.code() == "validation_failed", "got {err:?}");
    }

    #[tokio::test]
    async fn revoke_until_expires_with_the_token() {
        let store = InMemorySessionStore::new();
        let now = now_unix_secs();
        store.revoke_until("jti-a", now + 1).await.unwrap();
        assert!(store.is_revoked("jti-a").await.unwrap());

        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        assert!(
            !store.is_revoked("jti-a").await.unwrap(),
            "a record past the token exp must not revoke anything"
        );
        assert_eq!(store.revoked_count(), 0, "lookup deletes the dead record");
    }

    #[tokio::test]
    async fn revoke_until_in_the_past_is_a_noop() {
        let store = InMemorySessionStore::new();
        store
            .revoke_until("jti-a", now_unix_secs().saturating_sub(10))
            .await
            .unwrap();
        assert!(!store.is_revoked("jti-a").await.unwrap());
        assert_eq!(store.revoked_count(), 0);
    }

    #[tokio::test]
    async fn purge_expired_removes_only_dead_records() {
        let store = InMemorySessionStore::new();
        let now = now_unix_secs();
        store.revoke("indefinite").await.unwrap();
        store.revoke_until("dead", now + 1).await.unwrap();
        store.revoke_until("live", now + 3600).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        assert_eq!(store.purge_expired(now_unix_secs()), 1);
        assert_eq!(store.revoked_count(), 2, "indefinite + live remain");
        assert!(store.is_revoked("indefinite").await.unwrap());
        assert!(store.is_revoked("live").await.unwrap());
    }

    #[tokio::test]
    async fn revoke_all_for_subject_is_not_implemented_by_default() {
        let err = InMemorySessionStore::new()
            .revoke_all_for_subject("user-1")
            .await
            .expect_err("the dependency-free store has no subject index");
        assert!(
            err.status_code() == 501,
            "an unsupported capability is 501, not a generic 500: {err:?}"
        );
        assert_eq!(err.status_code(), 501);
    }

    #[tokio::test]
    async fn revoke_by_refresh_family_is_not_implemented_by_default() {
        let store = InMemorySessionStore::new();
        assert!(
            !store.supports_refresh_family_revocation(),
            "the default capability declaration must match the 501 default"
        );
        let err = store
            .revoke_by_refresh_family("family-1")
            .await
            .expect_err("the dependency-free store has no refresh-family index");
        assert_eq!(
            err.status_code(),
            501,
            "an unsupported capability is 501, never a silent success: {err:?}"
        );
        assert!(
            err.public_detail()
                .is_some_and(|detail| detail.contains("revoke_by_refresh_family")),
            "the 501 must name the override: {err:?}"
        );
    }
}
