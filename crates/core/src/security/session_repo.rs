//! Server-side session repository for cookie authentication.
//!
//! A browser session is an **opaque id** stored in an `HttpOnly` cookie. Unlike
//! a JWT it has no client-visible claims: the server resolves the id through
//! [`ISessionRepository`] on every request. That is what makes logout and
//! "sign out everywhere" immediate, and why the cookie scheme does not reuse
//! the JWT revocation list (`ISessionStore`), which is keyed by `jti` and has
//! no session state to read.
//!
//! The trait is dependency-free; the in-memory implementation is suitable for
//! tests and single-process deployments. A persistent implementation lives in
//! the optional `rust-webx-auth-ef` crate (2.5 M2).

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One server-side browser session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    /// Opaque session id (the cookie value). Never a credential the client
    /// can decode or extend.
    pub id: String,
    /// Subject (`sub`) the session belongs to.
    pub subject: String,
    /// Optional display name, copied into [`crate::auth::IdentityUser`].
    pub name: Option<String>,
    /// Optional email, copied into [`crate::auth::IdentityUser`].
    pub email: Option<String>,
    /// Optional tenant, copied into the `tenant_id` claim.
    pub tenant_id: Option<String>,
    /// Roles captured at login. The session is the authority for these until
    /// it is rotated/deleted.
    pub roles: Vec<String>,
    /// Permissions captured at login.
    pub permissions: Vec<String>,
    /// Creation time (Unix seconds).
    pub issued_at_unix_secs: u64,
    /// Absolute expiry (Unix seconds). The session dies at this instant even if
    /// it is active.
    pub expires_at_unix_secs: u64,
    /// Last time the session was seen (for idle timeout / sliding expiry).
    pub last_seen_unix_secs: u64,
}

impl SessionRecord {
    /// A session for `subject` created at `issued_at_unix_secs`, expiring at
    /// `expires_at_unix_secs`.
    pub fn new(
        id: impl Into<String>,
        subject: impl Into<String>,
        issued_at_unix_secs: u64,
        expires_at_unix_secs: u64,
    ) -> Self {
        Self {
            id: id.into(),
            subject: subject.into(),
            name: None,
            email: None,
            tenant_id: None,
            roles: Vec::new(),
            permissions: Vec::new(),
            issued_at_unix_secs,
            expires_at_unix_secs,
            last_seen_unix_secs: issued_at_unix_secs,
        }
    }

    /// Set the display name claim.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the email claim.
    pub fn with_email(mut self, email: impl Into<String>) -> Self {
        self.email = Some(email.into());
        self
    }

    /// Set the tenant claim.
    pub fn with_tenant(mut self, tenant: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant.into());
        self
    }

    /// Set the role list.
    pub fn with_roles<I, S>(mut self, roles: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.roles = roles.into_iter().map(Into::into).collect();
        self
    }

    /// Set the permission list.
    pub fn with_permissions<I, S>(mut self, permissions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.permissions = permissions.into_iter().map(Into::into).collect();
        self
    }

    /// Whether the absolute expiry has passed at `now_unix_secs`.
    pub fn is_expired(&self, now_unix_secs: u64) -> bool {
        now_unix_secs >= self.expires_at_unix_secs
    }

    /// Whether the session has been idle longer than `idle_timeout`.
    ///
    /// A zero timeout disables the idle check; a session with a `last_seen`
    /// in the future (clock skew) is treated as fresh, not expired.
    pub fn is_idle_expired(&self, now_unix_secs: u64, idle_timeout: Duration) -> bool {
        if idle_timeout.is_zero() {
            return false;
        }
        now_unix_secs.saturating_sub(self.last_seen_unix_secs) > idle_timeout.as_secs()
    }
}

/// Storage for browser sessions.
///
/// Errors are **not swallowed**: if the repository cannot answer, the cookie
/// scheme fails the request instead of downgrading to anonymous. The
/// authentication middleware maps the error through the host's problem+json
/// writer, so a database outage is a `500/503`, not a silent logout.
#[async_trait::async_trait]
pub trait ISessionRepository: Send + Sync {
    /// Persist a new session. An existing id must be replaced (rotation writes
    /// the new record before deleting the old one).
    async fn create(&self, record: SessionRecord) -> Result<()>;

    /// Load a session by id; `None` for unknown ids.
    async fn get(&self, id: &str) -> Result<Option<SessionRecord>>;

    /// Record activity for sliding expiry. Idempotent; unknown ids are ignored.
    async fn touch(&self, id: &str, now_unix_secs: u64) -> Result<()>;

    /// Delete one session (logout). Unknown ids are not an error.
    async fn delete(&self, id: &str) -> Result<()>;

    /// Delete every session of `subject` ("sign out everywhere").
    ///
    /// The default returns `501 Not Implemented` rather than pretending a
    /// backend that cannot enumerate subject sessions performed the logout.
    async fn delete_all_for_subject(&self, subject: &str) -> Result<u64> {
        Err(Error::Status(
            501,
            format!(
                "this ISessionRepository cannot delete every session of '{subject}'; override delete_all_for_subject"
            ),
        ))
    }
}

/// Process-local [`ISessionRepository`] for tests and single-process hosts.
///
/// [`get`](ISessionRepository::get) is O(1) (no map-wide scan); expired ids are
/// filtered on read and the cookie scheme deletes the record it just rejected.
/// Call [`purge_expired`](Self::purge_expired) periodically for ids that are
/// never read again. Revocations still do not survive a restart.
#[derive(Debug, Default)]
pub struct InMemorySessionRepository {
    sessions: RwLock<HashMap<String, SessionRecord>>,
}

impl InMemorySessionRepository {
    /// Create an empty repository.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of stored (possibly expired but not yet purged) sessions.
    pub fn len(&self) -> usize {
        self.sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Remove every expired record and return how many were removed.
    ///
    /// [`get`](ISessionRepository::get) deliberately does **not** scan the whole
    /// map: the authentication hot path must stay O(1). Expired records that are
    /// read are deleted by the cookie scheme; this method is the maintenance
    /// sweep for ids that are never read again.
    pub fn purge_expired(&self, now_unix_secs: u64) -> usize {
        let mut sessions = self
            .sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let before = sessions.len();
        sessions.retain(|_, record| !record.is_expired(now_unix_secs));
        before - sessions.len()
    }

    /// Whether the repository holds no sessions.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

#[async_trait::async_trait]
impl ISessionRepository for InMemorySessionRepository {
    async fn create(&self, record: SessionRecord) -> Result<()> {
        if record.id.trim().is_empty() {
            return Err(Error::Validation(
                "a session id must not be empty".to_string(),
            ));
        }
        self.sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(record.id.clone(), record);
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<SessionRecord>> {
        let now = now_unix_secs();
        // O(1): no map-wide scan on the authentication hot path. Expired ids
        // are filtered here and deleted by the caller when it sees the miss;
        // `purge_expired` handles ids nobody reads again.
        Ok(self
            .sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .filter(|record| !record.is_expired(now))
            .cloned())
    }

    async fn touch(&self, id: &str, now_unix_secs: u64) -> Result<()> {
        if let Some(record) = self
            .sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(id)
        {
            record.last_seen_unix_secs = now_unix_secs;
        }
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        self.sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        Ok(())
    }

    async fn delete_all_for_subject(&self, subject: &str) -> Result<u64> {
        let mut sessions = self
            .sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let before = sessions.len();
        sessions.retain(|_, record| record.subject != subject);
        Ok((before - sessions.len()) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, subject: &str) -> SessionRecord {
        let now = now_unix_secs();
        SessionRecord::new(id, subject, now, now + 3600)
    }

    #[tokio::test]
    async fn create_get_and_delete_round_trip() {
        let repo = InMemorySessionRepository::new();
        repo.create(record("s1", "user-1")).await.unwrap();

        let loaded = repo.get("s1").await.unwrap().expect("session exists");
        assert_eq!(loaded.subject, "user-1");
        assert_eq!(repo.len(), 1);

        repo.delete("s1").await.unwrap();
        assert!(repo.get("s1").await.unwrap().is_none());
        assert!(repo.is_empty());
    }

    #[tokio::test]
    async fn empty_id_is_rejected() {
        let repo = InMemorySessionRepository::new();
        let error = repo.create(record("  ", "user-1")).await.unwrap_err();
        assert!(matches!(error, Error::Validation(_)), "got {error:?}");
    }

    #[tokio::test]
    async fn expired_sessions_are_not_returned_and_purged_explicitly() {
        let repo = InMemorySessionRepository::new();
        let now = now_unix_secs();
        repo.create(SessionRecord::new(
            "old",
            "user-1",
            now,
            now.saturating_sub(1),
        ))
        .await
        .unwrap();
        assert!(repo.get("old").await.unwrap().is_none());
        assert_eq!(
            repo.len(),
            1,
            "get must stay O(1) and not scan the whole map"
        );
        assert_eq!(repo.purge_expired(now), 1);
        assert!(repo.is_empty());
    }

    #[tokio::test]
    async fn touch_updates_last_seen_and_delete_all_removes_subject_sessions() {
        let repo = InMemorySessionRepository::new();
        repo.create(record("s1", "user-1")).await.unwrap();
        repo.create(record("s2", "user-1")).await.unwrap();
        repo.create(record("s3", "user-2")).await.unwrap();

        repo.touch("s1", 1_000).await.unwrap();
        let loaded = repo.get("s1").await.unwrap().unwrap();
        assert_eq!(loaded.last_seen_unix_secs, 1_000);

        let removed = repo.delete_all_for_subject("user-1").await.unwrap();
        assert_eq!(removed, 2);
        assert!(repo.get("s1").await.unwrap().is_none());
        assert!(repo.get("s3").await.unwrap().is_some());
    }

    #[test]
    fn idle_expiry_compares_against_the_timeout() {
        let now = 10_000;
        let record = SessionRecord::new("s", "u", now, now + 1000);
        assert!(!record.is_idle_expired(now + 10, Duration::from_secs(30)));
        assert!(record.is_idle_expired(now + 31, Duration::from_secs(30)));
        assert!(
            !record.is_idle_expired(now + 10_000, Duration::ZERO),
            "zero disables the idle check"
        );
    }
}
