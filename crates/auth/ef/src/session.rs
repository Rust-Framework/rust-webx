//! Durable browser-session storage (`auth_sessions`).
//!
//! `EfSessionRepository` implements
//! [`ISessionRepository`] over
//! `rust-ef`: cookie sessions survive process restarts and are shared by every
//! instance. Session ids are opaque; roles/permissions are stored as JSON and
//! parsed on read (corrupt rows authenticate with empty roles, never elevated).
//!
//! `ensure_created` is for tests only; production uses the official `rust-ef
//! migration` tooling.

use std::sync::Arc;

use async_trait::async_trait;
use rust_ef::prelude::*;
use webx_core::error::{Error, Result};
use webx_core::security::{ISessionRepository, SessionRecord};

use crate::{to_i64, unix_now, DbContextFactory};

/// One `auth_sessions` row.
#[derive(Debug, Clone, EntityType)]
#[table("auth_sessions")]
pub struct SessionEntity {
    /// Opaque session id (cookie value).
    #[primary_key]
    #[max_length(64)]
    pub id: String,
    /// Subject (`sub`) the session belongs to.
    #[required]
    #[max_length(128)]
    #[index]
    pub subject: String,
    #[max_length(256)]
    pub name: Option<String>,
    #[max_length(256)]
    pub email: Option<String>,
    #[max_length(128)]
    pub tenant_id: Option<String>,
    /// JSON array of roles captured at login.
    #[required]
    pub roles_json: String,
    /// JSON array of permissions captured at login.
    #[required]
    pub permissions_json: String,
    #[required]
    pub issued_at_unix_secs: i64,
    #[required]
    #[index]
    pub expires_at_unix_secs: i64,
    #[required]
    pub last_seen_unix_secs: i64,
}

impl SessionEntity {
    fn from_record(record: &SessionRecord) -> Self {
        Self {
            id: record.id.clone(),
            subject: record.subject.clone(),
            name: record.name.clone(),
            email: record.email.clone(),
            tenant_id: record.tenant_id.clone(),
            roles_json: serde_json::to_string(&record.roles).unwrap_or_else(|_| "[]".into()),
            permissions_json: serde_json::to_string(&record.permissions)
                .unwrap_or_else(|_| "[]".into()),
            issued_at_unix_secs: to_i64(record.issued_at_unix_secs),
            expires_at_unix_secs: to_i64(record.expires_at_unix_secs),
            last_seen_unix_secs: to_i64(record.last_seen_unix_secs),
        }
    }

    fn to_record(&self) -> SessionRecord {
        SessionRecord {
            id: self.id.clone(),
            subject: self.subject.clone(),
            name: self.name.clone(),
            email: self.email.clone(),
            tenant_id: self.tenant_id.clone(),
            roles: serde_json::from_str(&self.roles_json).unwrap_or_default(),
            permissions: serde_json::from_str(&self.permissions_json).unwrap_or_default(),
            issued_at_unix_secs: from_i64(self.issued_at_unix_secs),
            expires_at_unix_secs: from_i64(self.expires_at_unix_secs),
            last_seen_unix_secs: from_i64(self.last_seen_unix_secs),
        }
    }
}

/// Durable [`ISessionRepository`] over `rust-ef`.
pub struct EfSessionRepository {
    factory: DbContextFactory,
}

impl EfSessionRepository {
    /// Build over a context factory.
    pub fn new(factory: DbContextFactory) -> Self {
        Self { factory }
    }

    /// Wrap configured `DbContextOptions`.
    pub fn from_options(options: Arc<DbContextOptions>) -> Self {
        Self::new(Arc::new(move || DbContext::from_options(&options)))
    }

    /// Delete rows whose absolute expiry has passed; returns the count.
    pub async fn purge_expired(&self) -> Result<u64> {
        let mut ctx = (self.factory)().map_err(unavailable)?;
        let now = to_i64(unix_now());
        let removed = linq!(ctx.set::<SessionEntity>(), |row: SessionEntity| row
            .expires_at_unix_secs
            <= now)
        .execute_delete()
        .await
        .map_err(unavailable)?;
        Ok(removed)
    }
}

#[async_trait]
impl ISessionRepository for EfSessionRepository {
    async fn create(&self, record: SessionRecord) -> Result<()> {
        if record.id.trim().is_empty() {
            return Err(Error::Validation(
                "a session id must not be empty".to_string(),
            ));
        }
        let mut ctx = (self.factory)().map_err(unavailable)?;
        // Upsert: rotation writes the replacement row before deleting the old
        // one, and a retry of the same create must be idempotent.
        ctx.upsert(SessionEntity::from_record(&record));
        ctx.save_changes().await.map_err(unavailable)?;
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<SessionRecord>> {
        let mut ctx = (self.factory)().map_err(unavailable)?;
        let id = id.to_string();
        let now = to_i64(unix_now());
        let row = linq!(ctx.set::<SessionEntity>(), |row: SessionEntity| row.id
            == id
            && row.expires_at_unix_secs > now)
        .first_or_default()
        .await
        .map_err(unavailable)?;
        Ok(row.map(|row| row.to_record()))
    }

    async fn touch(&self, id: &str, now_unix_secs: u64) -> Result<()> {
        if id.trim().is_empty() {
            return Ok(());
        }
        let mut ctx = (self.factory)().map_err(unavailable)?;
        let id = id.to_string();
        linq!(ctx.set::<SessionEntity>(), |row: SessionEntity| row.id
            == id)
        .execute_update()
        .set_column(
            SessionEntity::COLUMN_LAST_SEEN_UNIX_SECS,
            to_i64(now_unix_secs),
        )
        .execute()
        .await
        .map_err(unavailable)?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let mut ctx = (self.factory)().map_err(unavailable)?;
        let id = id.to_string();
        linq!(ctx.set::<SessionEntity>(), |row: SessionEntity| row.id
            == id)
        .execute_delete()
        .await
        .map_err(unavailable)?;
        Ok(())
    }

    async fn delete_all_for_subject(&self, subject: &str) -> Result<u64> {
        let mut ctx = (self.factory)().map_err(unavailable)?;
        let subject = subject.to_string();
        let removed = linq!(ctx.set::<SessionEntity>(), |row: SessionEntity| row.subject
            == subject)
        .execute_delete()
        .await
        .map_err(unavailable)?;
        Ok(removed)
    }
}

fn unavailable(error: EFError) -> Error {
    Error::Status(503, format!("session store unavailable: {error}"))
}

fn from_i64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DB: AtomicU64 = AtomicU64::new(0);

    fn test_options() -> (Arc<DbContextOptions>, PathBuf) {
        let sequence = NEXT_DB.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "webx-auth-ef-session-{}-{}-{}.db",
            std::process::id(),
            unix_now(),
            sequence
        ));
        let mut builder = DbContextOptionsBuilder::new();
        builder.use_sqlite(&path.to_string_lossy());
        let options = Arc::new(builder.build());
        options.create_provider().expect("provider");
        (options, path)
    }

    async fn prepare(options: &Arc<DbContextOptions>) {
        let ctx = DbContext::from_options(options).expect("context");
        ctx.ensure_created().await.expect("schema");
    }

    fn record(id: &str, subject: &str, expires: u64) -> SessionRecord {
        SessionRecord::new(id, subject, unix_now(), expires)
            .with_name("Ada")
            .with_email("ada@example.com")
            .with_tenant("tenant-1")
            .with_roles(["admin", "user"])
            .with_permissions(["read"])
    }

    #[tokio::test]
    async fn create_get_touch_and_delete_round_trip() {
        let (options, path) = test_options();
        prepare(&options).await;
        let repo = EfSessionRepository::from_options(Arc::clone(&options));
        let now = unix_now();

        repo.create(record("s1", "user-1", now + 3600))
            .await
            .expect("create");
        let loaded = repo.get("s1").await.expect("get").expect("row");
        assert_eq!(loaded.subject, "user-1");
        assert_eq!(loaded.roles, vec!["admin".to_string(), "user".to_string()]);
        assert_eq!(loaded.permissions, vec!["read".to_string()]);

        repo.touch("s1", now + 10).await.expect("touch");
        assert_eq!(
            repo.get("s1").await.unwrap().unwrap().last_seen_unix_secs,
            now + 10
        );

        repo.delete("s1").await.expect("delete");
        assert!(repo.get("s1").await.unwrap().is_none());
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn expired_sessions_are_invisible_and_purgeable() {
        let (options, path) = test_options();
        prepare(&options).await;
        let repo = EfSessionRepository::from_options(Arc::clone(&options));
        let now = unix_now();
        repo.create(record("old", "user-1", now.saturating_sub(1)))
            .await
            .expect("create");
        assert!(repo.get("old").await.unwrap().is_none());
        assert_eq!(repo.purge_expired().await.unwrap(), 1);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn delete_all_for_subject_only_affects_that_subject() {
        let (options, path) = test_options();
        prepare(&options).await;
        let repo = EfSessionRepository::from_options(Arc::clone(&options));
        let now = unix_now();
        repo.create(record("a", "user-1", now + 60))
            .await
            .expect("a");
        repo.create(record("b", "user-2", now + 60))
            .await
            .expect("b");

        assert_eq!(repo.delete_all_for_subject("user-1").await.unwrap(), 1);
        assert!(repo.get("a").await.unwrap().is_none());
        assert!(repo.get("b").await.unwrap().is_some());
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn create_upserts_the_same_id() {
        let (options, path) = test_options();
        prepare(&options).await;
        let repo = EfSessionRepository::from_options(Arc::clone(&options));
        let now = unix_now();
        repo.create(record("s1", "user-1", now + 60))
            .await
            .expect("create");
        repo.create(record("s1", "user-2", now + 60))
            .await
            .expect("upsert");
        assert_eq!(repo.get("s1").await.unwrap().unwrap().subject, "user-2");
        let _ = std::fs::remove_file(path);
    }
}
