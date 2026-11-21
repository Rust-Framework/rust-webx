//! Refresh-token rotation state.
//!
//! A refresh token is an **opaque** `token_id.secret` string. Only the hash of
//! the secret is stored, so a database leak does not hand out sessions. Rotation
//! is an atomic compare-and-set: one caller can consume an active token, every
//! other caller observes `Used` — which the service treats as **reuse** and
//! revokes the whole family.
//!
//! The port is dependency-free; the in-memory implementation is for tests and
//! single-process deployments. A persistent implementation lives in the
//! optional `rust-webx-auth-ef` crate (M2).

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

/// Lifecycle of one refresh token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshTokenStatus {
    /// Can be rotated once.
    Active,
    /// Already rotated; another presentation is reuse.
    Used,
    /// Withdrawn by logout, reuse detection, or family revocation.
    Revoked,
}

/// One refresh token record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshTokenRecord {
    /// Public lookup id (the part before `.` in the presented token).
    pub token_id: String,
    /// Hash of the secret (hex). Never the secret itself.
    pub secret_hash: String,
    /// Family id shared by every rotation of one login.
    pub family_id: String,
    /// 0 for the first token, +1 for each rotation.
    pub generation: u64,
    /// Subject the session belongs to.
    pub subject: String,
    /// Optional tenant snapshot.
    pub tenant_id: Option<String>,
    /// Optional client binding (e.g. `"web"`).
    pub client_id: Option<String>,
    /// When this token was issued (Unix seconds).
    pub issued_at_unix_secs: u64,
    /// When this token expires (Unix seconds).
    pub expires_at_unix_secs: u64,
    /// When this token was consumed, if it was.
    pub used_at_unix_secs: Option<u64>,
    /// Current status.
    pub status: RefreshTokenStatus,
    /// When the family was created (absolute family lifetime).
    pub family_created_at_unix_secs: u64,
}

impl RefreshTokenRecord {
    /// Whether the token is past its own expiry.
    pub fn is_expired(&self, now_unix_secs: u64) -> bool {
        now_unix_secs >= self.expires_at_unix_secs
    }
}

/// Result of an atomic rotation attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshRotation {
    /// The active token was consumed and the replacement stored.
    Rotated,
    /// The token had already been used: the caller must treat it as reuse.
    Reused,
    /// The token or its family expired.
    Expired,
    /// The token or family was revoked.
    Revoked,
    /// Unknown token id (including a race that removed it).
    NotFound,
}

/// Persistence port for refresh tokens.
///
/// [`rotate`](Self::rotate) must be atomic: mark the old record `Used`, set
/// `used_at_unix_secs`, and insert the replacement in one critical section, or
/// return `Reused`/`Expired`/`Revoked` without inserting anything.
#[async_trait::async_trait]
pub trait IRefreshTokenStore: Send + Sync {
    /// Store a brand-new token (generation 0). Duplicate ids are an error.
    async fn insert(&self, record: RefreshTokenRecord) -> Result<()>;

    /// Look up a token by its public id.
    async fn get(&self, token_id: &str) -> Result<Option<RefreshTokenRecord>>;

    /// Atomically consume `token_id` and store `replacement`.
    async fn rotate(
        &self,
        token_id: &str,
        now_unix_secs: u64,
        replacement: RefreshTokenRecord,
    ) -> Result<RefreshRotation>;

    /// Revoke every token in `family_id`; returns how many changed.
    async fn revoke_family(&self, family_id: &str) -> Result<u64>;

    /// Revoke every token of `subject`; returns how many changed.
    async fn revoke_all_for_subject(&self, subject: &str) -> Result<u64>;
}

/// Process-local [`IRefreshTokenStore`].
#[derive(Debug, Default)]
pub struct InMemoryRefreshTokenStore {
    tokens: Mutex<HashMap<String, RefreshTokenRecord>>,
}

impl InMemoryRefreshTokenStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of records (diagnostics/tests).
    pub fn len(&self) -> usize {
        self.tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait::async_trait]
impl IRefreshTokenStore for InMemoryRefreshTokenStore {
    async fn insert(&self, record: RefreshTokenRecord) -> Result<()> {
        if record.token_id.trim().is_empty() {
            return Err(Error::Validation(
                "a refresh token id must not be empty".to_string(),
            ));
        }
        let mut tokens = self.tokens.lock().unwrap_or_else(PoisonError::into_inner);
        if tokens.contains_key(&record.token_id) {
            return Err(Error::Internal(
                "refresh token id collision; retry token generation".to_string(),
            ));
        }
        tokens.insert(record.token_id.clone(), record);
        Ok(())
    }

    async fn get(&self, token_id: &str) -> Result<Option<RefreshTokenRecord>> {
        Ok(self
            .tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(token_id)
            .cloned())
    }

    async fn rotate(
        &self,
        token_id: &str,
        now_unix_secs: u64,
        replacement: RefreshTokenRecord,
    ) -> Result<RefreshRotation> {
        let mut tokens = self.tokens.lock().unwrap_or_else(PoisonError::into_inner);

        let family_id = match tokens.get(token_id) {
            None => return Ok(RefreshRotation::NotFound),
            Some(record) => record.family_id.clone(),
        };

        // A revoked family wins over every other state.
        let family_revoked = tokens.values().any(|record| {
            record.family_id == family_id && record.status == RefreshTokenStatus::Revoked
        });
        if family_revoked {
            return Ok(RefreshRotation::Revoked);
        }

        if tokens.contains_key(&replacement.token_id) {
            return Err(Error::Internal(
                "refresh token id collision; retry token generation".to_string(),
            ));
        }

        let Some(record) = tokens.get_mut(token_id) else {
            return Ok(RefreshRotation::NotFound);
        };
        match record.status {
            RefreshTokenStatus::Used => return Ok(RefreshRotation::Reused),
            RefreshTokenStatus::Revoked => return Ok(RefreshRotation::Revoked),
            RefreshTokenStatus::Active => {}
        }
        if record.is_expired(now_unix_secs) {
            // Do not mark the record revoked: an expired token must not make
            // the whole family look revoked. The absolute expiry ends it.
            return Ok(RefreshRotation::Expired);
        }

        record.status = RefreshTokenStatus::Used;
        record.used_at_unix_secs = Some(now_unix_secs);
        tokens.insert(replacement.token_id.clone(), replacement);
        Ok(RefreshRotation::Rotated)
    }

    async fn revoke_family(&self, family_id: &str) -> Result<u64> {
        let mut tokens = self.tokens.lock().unwrap_or_else(PoisonError::into_inner);
        let mut changed = 0;
        for record in tokens.values_mut() {
            if record.family_id == family_id && record.status != RefreshTokenStatus::Revoked {
                record.status = RefreshTokenStatus::Revoked;
                changed += 1;
            }
        }
        Ok(changed)
    }

    async fn revoke_all_for_subject(&self, subject: &str) -> Result<u64> {
        let mut tokens = self.tokens.lock().unwrap_or_else(PoisonError::into_inner);
        let mut changed = 0;
        for record in tokens.values_mut() {
            if record.subject == subject && record.status != RefreshTokenStatus::Revoked {
                record.status = RefreshTokenStatus::Revoked;
                changed += 1;
            }
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, family: &str, now: u64) -> RefreshTokenRecord {
        RefreshTokenRecord {
            token_id: id.to_string(),
            secret_hash: "hash".to_string(),
            family_id: family.to_string(),
            generation: 0,
            subject: "user-1".to_string(),
            tenant_id: None,
            client_id: None,
            issued_at_unix_secs: now,
            expires_at_unix_secs: now + 3600,
            used_at_unix_secs: None,
            status: RefreshTokenStatus::Active,
            family_created_at_unix_secs: now,
        }
    }

    #[tokio::test]
    async fn rotate_consumes_the_old_token_and_stores_the_new_one() {
        let store = InMemoryRefreshTokenStore::new();
        store.insert(record("t1", "f1", 100)).await.unwrap();

        let outcome = store
            .rotate("t1", 110, record("t2", "f1", 110))
            .await
            .unwrap();
        assert_eq!(outcome, RefreshRotation::Rotated);

        let old = store.get("t1").await.unwrap().unwrap();
        assert_eq!(old.status, RefreshTokenStatus::Used);
        assert_eq!(old.used_at_unix_secs, Some(110));
        let new = store.get("t2").await.unwrap().unwrap();
        assert_eq!(new.status, RefreshTokenStatus::Active);
    }

    #[tokio::test]
    async fn reusing_a_used_token_reports_reuse() {
        let store = InMemoryRefreshTokenStore::new();
        store.insert(record("t1", "f1", 100)).await.unwrap();
        store
            .rotate("t1", 110, record("t2", "f1", 110))
            .await
            .unwrap();

        let outcome = store
            .rotate("t1", 120, record("t3", "f1", 120))
            .await
            .unwrap();
        assert_eq!(outcome, RefreshRotation::Reused);
        assert!(store.get("t3").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn revoked_family_blocks_every_rotation() {
        let store = InMemoryRefreshTokenStore::new();
        store.insert(record("t1", "f1", 100)).await.unwrap();
        store
            .rotate("t1", 110, record("t2", "f1", 110))
            .await
            .unwrap();
        assert_eq!(store.revoke_family("f1").await.unwrap(), 2);

        let outcome = store
            .rotate("t2", 120, record("t3", "f1", 120))
            .await
            .unwrap();
        assert_eq!(outcome, RefreshRotation::Revoked);
    }

    #[tokio::test]
    async fn expired_token_is_rejected_without_marking_it_revoked() {
        let store = InMemoryRefreshTokenStore::new();
        store.insert(record("t1", "f1", 100)).await.unwrap();
        let outcome = store
            .rotate("t1", 10_000, record("t2", "f1", 10_000))
            .await
            .unwrap();
        assert_eq!(outcome, RefreshRotation::Expired);
        assert_eq!(
            store.get("t1").await.unwrap().unwrap().status,
            RefreshTokenStatus::Active,
            "expiry is not family revocation"
        );
    }

    #[tokio::test]
    async fn revoke_all_for_subject_only_touches_that_subject() {
        let store = InMemoryRefreshTokenStore::new();
        store.insert(record("t1", "f1", 100)).await.unwrap();
        let mut other = record("t2", "f2", 100);
        other.subject = "user-2".to_string();
        store.insert(other).await.unwrap();

        assert_eq!(store.revoke_all_for_subject("user-1").await.unwrap(), 1);
        assert_eq!(
            store.get("t1").await.unwrap().unwrap().status,
            RefreshTokenStatus::Revoked
        );
        assert_eq!(
            store.get("t2").await.unwrap().unwrap().status,
            RefreshTokenStatus::Active
        );
    }

    #[tokio::test]
    async fn duplicate_id_is_rejected() {
        let store = InMemoryRefreshTokenStore::new();
        store.insert(record("t1", "f1", 100)).await.unwrap();
        assert!(store.insert(record("t1", "f1", 100)).await.is_err());
    }
}
