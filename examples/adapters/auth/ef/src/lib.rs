//! Durable refresh-token storage for rust-webx.
//!
//! [`EfRefreshTokenStore`] implements
//! [`IRefreshTokenStore`] over
//! `rust-ef`. Rotation is a **conditional bulk update** (`status = 'active'`
//! guard) plus the replacement insert inside one transaction, so concurrent
//! refreshes cannot both win regardless of the database isolation level.
//!
//! The entity is migration-ready; `ensure_created` is for tests only. In
//! production generate DDL through the official `rust-ef migration` tooling.
//!
//! # Model registration
//!
//! `RefreshTokenEntity` derives `EntityType`, so a **default** `DbContext`
//! discovers it from the compile-time `inventory` registry without any call.
//! Call [`register_refresh_token_model`] when:
//!
//! - the context is **keyed** (`DbContextOptionsBuilder::context_key(..)`): a
//!   keyed model only discovers entities tagged `#[context("key")]`, and this
//!   function adds this entity explicitly; or
//! - a starter wants an explicit, idempotent registration step before model-based
//!   schema creation (`bootstrap` / official DDL diff), exactly like
//!   `rust_webx_idempotency_ef::register_idempotency_model`.
//!
//! ```ignore
//! use rust_ef::db_context::DbContext;
//! use webx_auth_ef::register_refresh_token_model;
//!
//! let mut ctx = DbContext::from_options(&keyed_options)?; // context_key("analytics")
//! register_refresh_token_model(&mut ctx);                 // auth_refresh_tokens
//! ctx.bootstrap().await?;                                 // model-based DDL
//! ```
//!
//! `ctx.migrate("Migrations")` reads versioned **SQL** files and does not read
//! the model, so registration is irrelevant to that path. The relative order of
//! `register_refresh_token_model` and another starter's `register_*` call does
//! not matter; registration is idempotent.
//!
//! ```ignore
//! let store = std::sync::Arc::new(EfRefreshTokenStore::from_options(db_options));
//! let service = webx_host::refresh::RefreshTokenService::new(
//!     store,
//!     Duration::from_secs(30 * 24 * 3600),
//!     Duration::from_secs(90 * 24 * 3600),
//! );
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use rust_ef::prelude::*;
use webx_core::error::{Error, Result};
use webx_core::security::{
    IRefreshTokenStore, RefreshRotation, RefreshTokenRecord, RefreshTokenStatus,
};

const STATUS_ACTIVE: &str = "active";
const STATUS_USED: &str = "used";
const STATUS_REVOKED: &str = "revoked";

/// One refresh-token row.
#[derive(Debug, Clone, EntityType)]
#[table("auth_refresh_tokens")]
pub struct RefreshTokenEntity {
    /// Public lookup id (`token_id.secret` before the dot).
    #[primary_key]
    #[max_length(64)]
    pub token_id: String,
    /// SHA-256 of the secret, lowercase hex. Never the secret.
    #[required]
    #[max_length(128)]
    pub secret_hash: String,
    /// Family shared by every rotation of one login.
    #[required]
    #[max_length(64)]
    #[index]
    pub family_id: String,
    /// 0 for the first token, +1 per rotation.
    #[required]
    pub generation: i64,
    /// Subject / user id.
    #[required]
    #[max_length(128)]
    #[index]
    pub subject: String,
    /// Optional tenant snapshot.
    #[max_length(128)]
    pub tenant_id: Option<String>,
    /// Optional client binding.
    #[max_length(64)]
    pub client_id: Option<String>,
    /// Issued at (Unix seconds).
    #[required]
    pub issued_at_unix_secs: i64,
    /// Absolute expiry of this token (Unix seconds).
    #[required]
    #[index]
    pub expires_at_unix_secs: i64,
    /// Consumed at, when used.
    pub used_at_unix_secs: Option<i64>,
    /// `active` / `used` / `revoked`.
    #[required]
    #[max_length(16)]
    pub status: String,
    /// Family creation (absolute family lifetime).
    #[required]
    pub family_created_at_unix_secs: i64,
}

impl RefreshTokenEntity {
    fn from_record(record: &RefreshTokenRecord) -> Self {
        Self {
            token_id: record.token_id.clone(),
            secret_hash: record.secret_hash.clone(),
            family_id: record.family_id.clone(),
            generation: to_i64(record.generation),
            subject: record.subject.clone(),
            tenant_id: record.tenant_id.clone(),
            client_id: record.client_id.clone(),
            issued_at_unix_secs: to_i64(record.issued_at_unix_secs),
            expires_at_unix_secs: to_i64(record.expires_at_unix_secs),
            used_at_unix_secs: record.used_at_unix_secs.map(to_i64),
            status: status_name(record.status).to_string(),
            family_created_at_unix_secs: to_i64(record.family_created_at_unix_secs),
        }
    }

    fn to_record(&self) -> RefreshTokenRecord {
        RefreshTokenRecord {
            token_id: self.token_id.clone(),
            secret_hash: self.secret_hash.clone(),
            family_id: self.family_id.clone(),
            generation: u64::try_from(self.generation).unwrap_or(0),
            subject: self.subject.clone(),
            tenant_id: self.tenant_id.clone(),
            client_id: self.client_id.clone(),
            issued_at_unix_secs: u64::try_from(self.issued_at_unix_secs).unwrap_or(0),
            expires_at_unix_secs: u64::try_from(self.expires_at_unix_secs).unwrap_or(0),
            used_at_unix_secs: self
                .used_at_unix_secs
                .and_then(|value| u64::try_from(value).ok()),
            status: parse_status(&self.status),
            family_created_at_unix_secs: u64::try_from(self.family_created_at_unix_secs)
                .unwrap_or(0),
        }
    }
}

/// Register [`RefreshTokenEntity`] in the application `DbContext` model.
///
/// Call it from the same place the application registers its own entities
/// (`ctx.set::<T>()`), before model-based schema creation (`bootstrap` / DDL
/// diff) — the same step (and ordering rule) as
/// `rust_webx_idempotency_ef::register_idempotency_model`. A **default** context
/// already discovers the entity through `inventory`; a **keyed** context needs
/// this explicit call because it only discovers entities tagged
/// `#[context("key")]`. `ctx.migrate(dir)` reads SQL files and does not use the
/// model. Registration is idempotent, so a repeated call is harmless.
pub fn register_refresh_token_model(ctx: &mut DbContext) {
    ctx.set::<RefreshTokenEntity>();
}

/// A factory returning a fresh `DbContext` for one operation.
pub type DbContextFactory = Arc<dyn Fn() -> EFResult<DbContext> + Send + Sync>;

pub mod session;

pub use session::{EfSessionRepository, SessionEntity};

/// Durable [`IRefreshTokenStore`] over `rust-ef`.
pub struct EfRefreshTokenStore {
    factory: DbContextFactory,
}

impl EfRefreshTokenStore {
    /// Build over a context factory.
    pub fn new(factory: DbContextFactory) -> Self {
        Self { factory }
    }

    /// Wrap configured `DbContextOptions` (the common case).
    pub fn from_options(options: Arc<DbContextOptions>) -> Self {
        Self::new(Arc::new(move || DbContext::from_options(&options)))
    }
}

#[async_trait]
impl IRefreshTokenStore for EfRefreshTokenStore {
    async fn insert(&self, record: RefreshTokenRecord) -> Result<()> {
        if record.token_id.trim().is_empty() {
            return Err(Error::validation(
                "a refresh token id must not be empty".to_string(),
            ));
        }
        let mut ctx = (self.factory)().map_err(unavailable)?;
        ctx.add(RefreshTokenEntity::from_record(&record));
        ctx.save_changes().await.map_err(|error| match error {
            EFError::UniqueViolation { .. } => {
                Error::internal("refresh token id collision; retry token generation".to_string())
            }
            other => unavailable(other),
        })?;
        Ok(())
    }

    async fn get(&self, token_id: &str) -> Result<Option<RefreshTokenRecord>> {
        let mut ctx = (self.factory)().map_err(unavailable)?;
        let token_id = token_id.to_string();
        let row = linq!(
            ctx.set::<RefreshTokenEntity>(),
            |row: RefreshTokenEntity| row.token_id == token_id
        )
        .first_or_default()
        .await
        .map_err(unavailable)?;
        Ok(row.map(|row| row.to_record()))
    }

    async fn rotate(
        &self,
        token_id: &str,
        now_unix_secs: u64,
        replacement: RefreshTokenRecord,
    ) -> Result<RefreshRotation> {
        let mut ctx = (self.factory)().map_err(unavailable)?;
        let token_id = token_id.to_string();
        let now = to_i64(now_unix_secs);
        let replacement = RefreshTokenEntity::from_record(&replacement);

        let outcome = ctx
            .use_transaction(|ctx| {
                let token_id = token_id.clone();
                Box::pin(async move {
                    let query_id = token_id.clone();
                    let row = linq!(
                        ctx.set::<RefreshTokenEntity>(),
                        |row: RefreshTokenEntity| row.token_id == query_id
                    )
                    .first_or_default()
                    .await?;
                    let Some(row) = row else {
                        return Ok(RefreshRotation::NotFound);
                    };
                    match row.status.as_str() {
                        STATUS_USED => return Ok(RefreshRotation::Reused),
                        STATUS_REVOKED => return Ok(RefreshRotation::Revoked),
                        _ => {}
                    }
                    if row.expires_at_unix_secs <= now {
                        return Ok(RefreshRotation::Expired);
                    }

                    // Compare-and-set: only an `active` row may be consumed.
                    let guard = token_id.clone();
                    let active = STATUS_ACTIVE.to_string();
                    let affected = linq!(
                        ctx.set::<RefreshTokenEntity>(),
                        |row: RefreshTokenEntity| row.token_id == guard && row.status == active
                    )
                    .execute_update()
                    .set_column(RefreshTokenEntity::COLUMN_STATUS, STATUS_USED)
                    .set_column(RefreshTokenEntity::COLUMN_USED_AT_UNIX_SECS, now)
                    .execute()
                    .await?;

                    if affected == 0 {
                        // Lost the race: re-read to classify (Used = reuse,
                        // Revoked = family/forced logout).
                        let again = linq!(
                            ctx.set::<RefreshTokenEntity>(),
                            |row: RefreshTokenEntity| row.token_id == token_id
                        )
                        .first_or_default()
                        .await?;
                        return Ok(match again.map(|row| row.status) {
                            None => RefreshRotation::NotFound,
                            Some(status) if status == STATUS_USED => RefreshRotation::Reused,
                            Some(status) if status == STATUS_REVOKED => RefreshRotation::Revoked,
                            Some(_) => RefreshRotation::Reused,
                        });
                    }

                    ctx.add(replacement);
                    ctx.save_changes().await?;
                    Ok(RefreshRotation::Rotated)
                })
            })
            .await
            .map_err(unavailable)?;

        Ok(outcome)
    }

    async fn revoke_family(&self, family_id: &str) -> Result<u64> {
        let mut ctx = (self.factory)().map_err(unavailable)?;
        let family_id = family_id.to_string();
        let revoked = STATUS_REVOKED.to_string();
        let affected = linq!(
            ctx.set::<RefreshTokenEntity>(),
            |row: RefreshTokenEntity| row.family_id == family_id && row.status != revoked
        )
        .execute_update()
        .set_column(RefreshTokenEntity::COLUMN_STATUS, STATUS_REVOKED)
        .execute()
        .await
        .map_err(unavailable)?;
        Ok(affected)
    }

    async fn revoke_all_for_subject(&self, subject: &str) -> Result<u64> {
        let mut ctx = (self.factory)().map_err(unavailable)?;
        let subject = subject.to_string();
        let revoked = STATUS_REVOKED.to_string();
        let affected = linq!(
            ctx.set::<RefreshTokenEntity>(),
            |row: RefreshTokenEntity| row.subject == subject && row.status != revoked
        )
        .execute_update()
        .set_column(RefreshTokenEntity::COLUMN_STATUS, STATUS_REVOKED)
        .execute()
        .await
        .map_err(unavailable)?;
        Ok(affected)
    }
}

fn unavailable(error: EFError) -> Error {
    Error::status(503, format!("refresh token store unavailable: {error}"))
}

pub(crate) fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

fn status_name(status: RefreshTokenStatus) -> &'static str {
    match status {
        RefreshTokenStatus::Active => STATUS_ACTIVE,
        RefreshTokenStatus::Used => STATUS_USED,
        RefreshTokenStatus::Revoked => STATUS_REVOKED,
    }
}

fn parse_status(status: &str) -> RefreshTokenStatus {
    match status {
        STATUS_ACTIVE => RefreshTokenStatus::Active,
        STATUS_USED => RefreshTokenStatus::Used,
        // Unknown/corrupt status must never authenticate.
        _ => RefreshTokenStatus::Revoked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DB: AtomicU64 = AtomicU64::new(0);

    fn test_options() -> (Arc<DbContextOptions>, PathBuf) {
        test_options_with_key(None)
    }

    /// Build a context with a keyed model. Entities declared with
    /// `#[context("key")]` are discovered through `inventory`; an entity
    /// without that attribute (like `RefreshTokenEntity`) is **not** part of a
    /// keyed model unless `register_refresh_token_model` registers it.
    fn test_options_with_key(context_key: Option<&str>) -> (Arc<DbContextOptions>, PathBuf) {
        let sequence = NEXT_DB.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "webx-auth-ef-refresh-{}-{}-{}.db",
            std::process::id(),
            now_unix_secs(),
            sequence
        ));
        let mut builder = DbContextOptionsBuilder::new();
        builder.use_sqlite(&path.to_string_lossy());
        if let Some(key) = context_key {
            builder.context_key(key);
        }
        let options = Arc::new(builder.build());
        options.create_provider().expect("provider");
        (options, path)
    }

    async fn prepare(options: &Arc<DbContextOptions>) {
        let ctx = DbContext::from_options(options).expect("context");
        ctx.ensure_created().await.expect("schema");
    }

    fn record(id: &str, family: &str, now: u64) -> RefreshTokenRecord {
        RefreshTokenRecord {
            token_id: id.to_string(),
            secret_hash: "hash".to_string(),
            family_id: family.to_string(),
            generation: 0,
            subject: "user-1".to_string(),
            tenant_id: Some("tenant-1".to_string()),
            client_id: None,
            issued_at_unix_secs: now,
            expires_at_unix_secs: now + 3600,
            used_at_unix_secs: None,
            status: RefreshTokenStatus::Active,
            family_created_at_unix_secs: now,
        }
    }

    #[tokio::test]
    async fn registration_adds_the_entity_to_a_keyed_model() {
        let (options, path) = test_options_with_key(Some("analytics"));
        let mut ctx = DbContext::from_options(&options).expect("context");

        // A keyed context only discovers entities tagged for that key. Without
        // registration, `RefreshTokenEntity` (no `#[context]`) is absent — this
        // is the assertion the default context cannot make, because there the
        // `inventory` discovery already includes it.
        assert!(
            ctx.model_builder()
                .find_entity::<RefreshTokenEntity>()
                .is_none(),
            "the keyed model must not contain the unregistered entity"
        );
        assert!(!ctx.entity_metas_contains::<RefreshTokenEntity>());

        register_refresh_token_model(&mut ctx);
        // Registration is idempotent: the model must not gain a second entry.
        register_refresh_token_model(&mut ctx);

        assert!(ctx.entity_metas_contains::<RefreshTokenEntity>());
        let meta = ctx
            .model_builder()
            .find_entity::<RefreshTokenEntity>()
            .expect("the registered entity is part of the model");
        assert_eq!(meta.table_name.as_ref(), "auth_refresh_tokens");

        // `bootstrap`/DDL build from the model: the registered entity must
        // materialize a table, which is the contract the starter entry exists for.
        ctx.bootstrap().await.expect("bootstrap");

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn insert_get_and_rotate_round_trip() {
        let (options, path) = test_options();
        prepare(&options).await;
        let store = EfRefreshTokenStore::from_options(Arc::clone(&options));

        store.insert(record("t1", "f1", 100)).await.expect("insert");
        let loaded = store.get("t1").await.expect("get").expect("row");
        assert_eq!(loaded.family_id, "f1");
        assert_eq!(loaded.status, RefreshTokenStatus::Active);

        let outcome = store
            .rotate("t1", 110, record("t2", "f1", 110))
            .await
            .expect("rotate");
        assert_eq!(outcome, RefreshRotation::Rotated);
        assert_eq!(
            store.get("t1").await.unwrap().unwrap().status,
            RefreshTokenStatus::Used
        );
        assert_eq!(
            store.get("t2").await.unwrap().unwrap().status,
            RefreshTokenStatus::Active
        );

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn reuse_reports_reused_and_family_revoke_wins() {
        let (options, path) = test_options();
        prepare(&options).await;
        let store = EfRefreshTokenStore::from_options(Arc::clone(&options));
        store.insert(record("t1", "f1", 100)).await.expect("insert");
        store
            .rotate("t1", 110, record("t2", "f1", 110))
            .await
            .expect("rotate");

        assert_eq!(
            store
                .rotate("t1", 120, record("t3", "f1", 120))
                .await
                .unwrap(),
            RefreshRotation::Reused
        );
        assert_eq!(store.revoke_family("f1").await.unwrap(), 2);
        assert_eq!(
            store
                .rotate("t2", 130, record("t4", "f1", 130))
                .await
                .unwrap(),
            RefreshRotation::Revoked
        );

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn revoke_all_for_subject_only_touches_that_subject() {
        let (options, path) = test_options();
        prepare(&options).await;
        let store = EfRefreshTokenStore::from_options(Arc::clone(&options));

        store.insert(record("a1", "fa", 100)).await.expect("insert");
        let mut other = record("b1", "fb", 100);
        other.subject = "user-2".to_string();
        store.insert(other).await.expect("insert");

        assert_eq!(store.revoke_all_for_subject("user-1").await.unwrap(), 1);
        assert_eq!(
            store.get("a1").await.unwrap().unwrap().status,
            RefreshTokenStatus::Revoked
        );
        assert_eq!(
            store.get("b1").await.unwrap().unwrap().status,
            RefreshTokenStatus::Active
        );

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn expired_token_is_expired_not_rotated() {
        let (options, path) = test_options();
        prepare(&options).await;
        let store = EfRefreshTokenStore::from_options(Arc::clone(&options));
        store.insert(record("t1", "f1", 100)).await.expect("insert");
        assert_eq!(
            store
                .rotate("t1", 10_000, record("t2", "f1", 10_000))
                .await
                .unwrap(),
            RefreshRotation::Expired
        );
        let _ = std::fs::remove_file(path);
    }

    fn now_unix_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default()
    }
}
